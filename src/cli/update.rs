use macros_rs::crashln;
use opm::helpers;
use std::{
    fs,
    io::{self, Read, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

/// Repository used when `--repo` is not given. Change this one constant to
/// point the updater at a different fork; `opm update --help` prints it.
pub const DEFAULT_REPO: &str = "h0dev/opm";

/// Release tag the workflow in `.github/workflows/release.yml` publishes to.
const RELEASE_TAG: &str = "latest";

const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Prefix of the temporary download written beside the target binary. The dot
/// keeps it out of `ls` while it exists, and the prefix lets a failed install
/// remove exactly its own leftovers.
const TEMP_PREFIX: &str = ".opm.update.";

/// A `<owner>/<name>` GitHub repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl Repo {
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    fn api_latest(&self) -> String {
        format!(
            "https://api.github.com/repos/{}/{}/releases/tags/{RELEASE_TAG}",
            self.owner, self.name
        )
    }

    fn asset_url(&self, asset: &str) -> String {
        asset_url(&self.slug(), asset)
    }
}

pub fn asset_url(repo: &str, asset: &str) -> String {
    format!("https://github.com/{repo}/releases/download/{RELEASE_TAG}/{asset}")
}

pub fn parse_repo(repo: &str) -> Result<Repo, String> {
    match repo.split('/').collect::<Vec<_>>().as_slice() {
        [owner, name] if !owner.is_empty() && !name.is_empty() => Ok(Repo {
            owner: owner.to_string(),
            name: name.to_string(),
        }),
        _ => Err(format!(
            "--repo must be <owner>/<name>, for example {DEFAULT_REPO}; got {repo:?}"
        )),
    }
}

fn normalize_os(os: &str) -> &str {
    match os {
        "linux" => "linux",
        "macos" | "darwin" | "mac" => "macos",
        other => other,
    }
}

fn normalize_arch(arch: &str) -> Result<&'static str, String> {
    match arch {
        "x86_64" | "amd64" => Ok("x86_64"),
        "aarch64" | "arm64" => Ok("aarch64"),
        other => Err(format!("no release asset is published for {other}")),
    }
}

/// Name of the release asset for a platform. macOS builds carry no webui
/// feature, so the webui preference only applies where such an asset exists.
pub fn asset_name(os: &str, arch: &str, webui: bool) -> Result<String, String> {
    let os = normalize_os(os);
    if os != "linux" && os != "macos" {
        return Err(format!("no release asset is published for {os}"));
    }
    let arch = normalize_arch(arch)?;

    Ok(match (os, arch, webui) {
        ("linux", arch, true) => format!("opm-linux-{arch}-webui"),
        (os, arch, _) => format!("opm-{os}-{arch}"),
    })
}

/// The asset for the platform this binary was built for.
pub fn host_asset_name() -> Result<String, String> {
    asset_name(std::env::consts::OS, std::env::consts::ARCH, cfg!(feature = "webui"))
}

fn parse_version(text: &str) -> Option<Vec<u64>> {
    let start = text.find(|c: char| c.is_ascii_digit())?;
    let digits: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = Vec::new();
    for part in digits.split('.').filter(|p| !p.is_empty()) {
        parts.push(part.parse::<u64>().ok()?);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts)
    }
}

/// Comparable form of a version string, so `2.0.10` sorts above `2.0.9`.
#[cfg(test)]
fn version_key(text: &str) -> Vec<u64> {
    let mut parts = parse_version(text).unwrap_or_default();
    while parts.last() == Some(&0) {
        parts.pop();
    }
    parts
}

/// Whether `available` is a later release than `current`. A build of the same
/// release (same tag, different git sha) is not newer.
pub fn version_is_newer(current: &str, available: &str) -> bool {
    let (Some(current), Some(available)) = (parse_version(current), parse_version(available)) else {
        return false;
    };
    let width = current.len().max(available.len());
    for i in 0..width {
        let a = available.get(i).copied().unwrap_or(0);
        let b = current.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

/// Whether the bytes are an ELF or a macOS Mach-O executable.
pub fn is_executable_image(bytes: &[u8]) -> bool {
    const ELF: &[u8] = b"\x7fELF";
    const MACH_O: [[u8; 4]; 4] = [
        [0xfe, 0xed, 0xfa, 0xce],
        [0xce, 0xfa, 0xed, 0xfe],
        [0xfe, 0xed, 0xfa, 0xcf],
        [0xcf, 0xfa, 0xed, 0xfe],
    ];

    bytes.starts_with(ELF) || MACH_O.iter().any(|magic| bytes.starts_with(magic))
}

/// Replaces the target binary or tells the user how to get the privileges.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallDecision {
    /// Temporary files are created in the target's own directory, so the
    /// final rename stays on one filesystem and is atomic.
    Replace,
    /// The directory cannot take a temporary file, so nothing is written;
    /// print this command and exit non-zero.
    NeedsPrivileges { command: String },
}

pub fn directory_is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!("{TEMP_PREFIX}probe.{}", std::process::id()));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

pub fn decide(target: &Path, writable: bool) -> InstallDecision {
    if writable {
        InstallDecision::Replace
    } else {
        InstallDecision::NeedsPrivileges {
            command: format!("sudo {} update --yes", binary_name(target)),
        }
    }
}

/// Name of the target binary: the directory is implied, the file is not.
pub fn binary_name(target: &Path) -> &str {
    target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("opm")
}

/// Write `bytes` beside `target`, then swap it in atomically and keep the old
/// binary as `<target>.old`. Nothing is left behind on any error path.
pub fn install_beside(target: &Path, bytes: &[u8]) -> Result<(), String> {
    if !is_executable_image(bytes) {
        return Err(format!(
            "downloaded file is not an executable (got {} bytes, no ELF/Mach-O header)",
            bytes.len()
        ));
    }

    let dir = target.parent().unwrap_or_else(|| Path::new("."));
    let temp = dir.join(format!("{TEMP_PREFIX}{}", std::process::id()));

    let write_temp = || -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::set_permissions(&temp, fs::Permissions::from_mode(0o755))
    };

    if let Err(err) = write_temp() {
        let _ = fs::remove_file(&temp);
        return Err(format!("cannot write {}: {err}", temp.display()));
    }

    let backup = backup_path(target);
    let result = fs::rename(target, &backup).and_then(|_| fs::rename(&temp, target));

    if let Err(err) = result {
        let _ = fs::remove_file(&temp);
        if backup.exists() && !target.exists() {
            let _ = fs::rename(&backup, target);
        }
        return Err(format!("cannot replace {}: {err}", target.display()));
    }

    Ok(())
}

pub fn backup_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    match target.parent() {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

pub fn running_binary() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|err| format!("cannot locate this binary: {err}"))?;
    fs::canonicalize(&exe).or(Ok(exe))
}

struct Release {
    tag: String,
    body: Option<String>,
}

/// Every release the workflow publishes is tagged `latest`, so the tag says
/// nothing about which build the assets hold. The body names the commit they
/// were built from as `**Commit:** <sha>`; that is what identifies a build.
fn commit_from_body(body: Option<&str>) -> Option<String> {
    let body = body?;
    let line = body
        .lines()
        .find(|line| line.contains("Commit:"))?
        .replace('*', "");
    let rest = line.split_once("Commit:")?.1.trim();
    let sha = rest.split_whitespace().next()?;
    let is_sha = sha.len() >= 7 && sha.chars().all(|c| c.is_ascii_hexdigit());
    is_sha.then(|| sha.to_ascii_lowercase())
}

impl Release {
    /// The release describes exactly this build, so re-installing it would
    /// download and swap in an identical binary.
    fn is_this_build(&self) -> bool {
        let local = env!("GIT_HASH").trim().to_ascii_lowercase();
        if local.is_empty() {
            return false;
        }

        match commit_from_body(self.body.as_deref()) {
            Some(remote) => remote.starts_with(&local) || local.starts_with(&remote),
            // No identity to compare: only a tag that names a version can
            // tell us the build is the same.
            None => {
                parse_version(&self.tag) == parse_version(env!("CARGO_PKG_VERSION"))
                    && self.tag.to_ascii_lowercase() != RELEASE_TAG
            }
        }
    }
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .connect_timeout(REQUEST_TIMEOUT)
        .user_agent(format!(
            "{}/{} (+https://github.com/{DEFAULT_REPO})",
            env!("CARGO_PKG_NAME"),
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .map_err(|err| format!("cannot create HTTP client: {err}"))
}

fn github_error(status: reqwest::StatusCode, repo: &Repo) -> String {
    match status.as_u16() {
        404 => format!(
            "release not found in {}. Check --repo (default {DEFAULT_REPO}) \
             and that the {RELEASE_TAG} tag has been published",
            repo.slug()
        ),
        403 | 429 => format!(
            "GitHub rate limit reached while reading {}; try again later",
            repo.slug()
        ),
        _ => format!("GitHub returned {} for {}", status.as_u16(), repo.slug()),
    }
}

fn fetch_release(client: &reqwest::blocking::Client, repo: &Repo) -> Result<Release, String> {
    let response = client
        .get(repo.api_latest())
        .send()
        .map_err(|err| format!("cannot reach GitHub: {err}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(github_error(status, repo));
    }

    let body: serde_json::Value = response
        .json()
        .map_err(|err| format!("unexpected response from GitHub: {err}"))?;

    let tag = body
        .get("tag_name")
        .and_then(|value| value.as_str())
        .unwrap_or(RELEASE_TAG)
        .to_string();

    Ok(Release {
        tag,
        body: body
            .get("body")
            .and_then(|value| value.as_str())
            .map(|body| body.to_string()),
    })
}

fn ensure_asset_exists(
    client: &reqwest::blocking::Client,
    repo: &Repo,
    asset: &str,
) -> Result<(), String> {
    let response = client
        .head(repo.asset_url(asset))
        .send()
        .map_err(|err| format!("cannot reach GitHub: {err}"))?;

    match response.status() {
        status if status.is_success() => Ok(()),
        status if status.as_u16() == 404 => Err(format!(
            "asset {asset} not found in {}; list the assets or pass --asset",
            repo.slug()
        )),
        status => Err(format!(
            "GitHub returned {} for {asset}",
            status.as_u16()
        )),
    }
}

fn download(client: &reqwest::blocking::Client, url: &str, asset: &str) -> Result<Vec<u8>, String> {
    let mut response = client
        .get(url)
        .send()
        .map_err(|err| format!("cannot download {asset}: {err}"))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!(
            "cannot download {asset}: GitHub returned {}",
            status.as_u16()
        ));
    }

    let mut bytes = Vec::new();
    response
        .read_to_end(&mut bytes)
        .map_err(|err| format!("cannot read {asset}: {err}"))?;

    if bytes.is_empty() {
        return Err(format!("download of {asset} was empty"));
    }

    Ok(bytes)
}

pub struct Options {
    pub check: bool,
    pub yes: bool,
    pub force: bool,
    pub asset: Option<String>,
    pub repo: String,
}

pub fn stdin_is_tty() -> bool {
    use std::io::IsTerminal;
    io::stdin().is_terminal()
}

fn confirm(prompt: &str) -> bool {
    print!("{prompt} [y/N] ");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    match io::stdin().read_line(&mut answer) {
        Ok(_) => matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes"),
        Err(_) => false,
    }
}

fn escalate(command: &str) -> ! {
    eprintln!(
        "{} {} is not writable, so nothing was changed.",
        *helpers::FAIL,
        "the directory holding this binary"
    );

    // The user asked for the update, so try the elevation ourselves rather than
    // leaving them to copy the command. If sudo is unavailable or refused we
    // print it, which is still the fallback.
    if sudo_available() {
        eprintln!("{} Re-running with sudo: {}", *helpers::INFO, command);
        restart_privileged(command);
    }

    eprintln!("{} Run: {}", *helpers::INFO, command);
    std::process::exit(1);
}

/// Whether a bare `sudo -n true` can run without prompting here.
fn sudo_available() -> bool {
    Command::new("sudo")
        .arg("-n")
        .arg("true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn restart_privileged(command: &str) -> ! {
    let mut parts = command.split_whitespace();
    let program = parts.next().unwrap_or("sudo");

    match Command::new(program)
        .args(parts.collect::<Vec<_>>())
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
    {
        Ok(status) if status.success() => std::process::exit(0),
        Ok(status) => {
            let code = status.code().unwrap_or(1);
            eprintln!("{} Re-running as root exited with {code}", *helpers::FAIL);
            std::process::exit(code);
        }
        Err(err) => {
            eprintln!("{} Cannot run {program}: {err}", *helpers::FAIL);
            std::process::exit(1);
        }
    }
}

pub fn run(options: Options) {
    let repo = match parse_repo(&options.repo) {
        Ok(repo) => repo,
        Err(err) => crashln!("{} {}", *helpers::FAIL, err),
    };

    let asset = match options.asset.clone().or_else(|| host_asset_name().ok()) {
        Some(asset) if !asset.trim().is_empty() => asset,
        _ => crashln!(
            "{} no release asset exists for {}/{}; pass --asset <name>",
            *helpers::FAIL,
            std::env::consts::OS,
            std::env::consts::ARCH
        ),
    };

    let client = match http_client() {
        Ok(client) => client,
        Err(err) => crashln!("{} {}", *helpers::FAIL, err),
    };

    let release = match fetch_release(&client, &repo) {
        Ok(release) => release,
        Err(err) => crashln!("{} {}", *helpers::FAIL, err),
    };

    let current = env!("CARGO_PKG_VERSION");
    let same_build = release.is_this_build();

    println!("{} checking {} for {asset}", *helpers::INFO, repo.slug());
    println!("  current version: {current} ({})", local_build());
    println!(
        "  available build: {}",
        commit_from_body(release.body.as_deref()).unwrap_or_else(|| release.tag.clone())
    );

    if let Err(err) = ensure_asset_exists(&client, &repo, &asset) {
        crashln!("{} {}", *helpers::FAIL, err);
    }
    println!("  asset: {}", repo.asset_url(&asset));

    if options.check {
        if same_build {
            println!(
                "{} this build already is the {asset} asset of {} ({})",
                *helpers::SUCCESS,
                release.tag,
                local_build()
            );
        } else if version_is_newer(current, &release.tag) {
            println!(
                "{} a newer release is available: {}",
                *helpers::WARN,
                release.tag
            );
        } else {
            println!(
                "{} a different build is available for {asset}",
                *helpers::WARN
            );
        }
        return;
    }

    if same_build && !options.force {
        println!(
            "{} already running {}, the latest {asset} is the same build ({})",
            *helpers::SUCCESS,
            current,
            local_build()
        );
        println!("{} use --force to reinstall it", *helpers::INFO);
        return;
    }

    let target = match running_binary() {
        Ok(target) => target,
        Err(err) => crashln!("{} {}", *helpers::FAIL, err),
    };
    let dir = target.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();

    let decision = decide(&target, directory_is_writable(&dir));
    if let InstallDecision::NeedsPrivileges { command } = decision {
        escalate(&command);
    }

    if !options.yes {
        if !stdin_is_tty() {
            crashln!(
                "{} {} would be replaced with the latest {asset}. Re-run with --yes",
                *helpers::FAIL,
                target.display()
            );
        }
        if !confirm(&format!(
            "Replace {} with the latest {asset}?",
            target.display()
        )) {
            println!("{} cancelled, nothing changed", *helpers::INFO);
            return;
        }
        // The user asked for the replacement, so this invocation may elevate.
        // Options are carried over so the sudo re-run keeps --asset/--repo.
        if !directory_is_writable(&dir) {
            restart_privileged(&format!("sudo {}", self_invocation(&options)));
        }
    }

    let bytes = match download(&client, &repo.asset_url(&asset), &asset) {
        Ok(bytes) => bytes,
        Err(err) => crashln!("{} {}", *helpers::FAIL, err),
    };

    if let Err(err) = install_beside(&target, &bytes) {
        if !directory_is_writable(&dir) {
            escalate(&format!("sudo {}", self_invocation(&options)));
        }
        crashln!("{} {}", *helpers::FAIL, err);
    }

    println!(
        "{} updated {} ({} bytes, previous kept at {})",
        *helpers::SUCCESS,
        target.display(),
        bytes.len(),
        backup_path(&target).display()
    );
    println!(
        "{} Restart the daemon to use it: {}",
        *helpers::WARN,
        "opm daemon restart"
    );
}

fn local_build() -> String {
    match env!("GIT_HASH").trim() {
        "" => "unknown build".to_string(),
        hash => format!("git {hash}"),
    }
}

/// This command line, so a privileged re-run does the same thing.
///
/// The binary is named by its own path, not as `opm`: a user may have installed
/// it under `~/.local/bin`, `/usr/local/bin`, or a name of their own, and the
/// re-run must replace the same file it is running from. Arguments are quoted
/// so a path or option value containing spaces survives.
fn self_invocation(options: &Options) -> String {
    let binary = running_binary()
        .map(|path| shell_quote(&path.to_string_lossy()))
        .unwrap_or_else(|_| "opm".to_string());

    let mut command = format!("{binary} update --yes");
    if options.force {
        command.push_str(" --force");
    }
    if let Some(asset) = &options.asset {
        command.push_str(&format!(" --asset {}", shell_quote(asset)));
    }
    if options.repo != DEFAULT_REPO {
        command.push_str(&format!(" --repo {}", shell_quote(&options.repo)));
    }
    command
}

/// Single-quote a value for a shell, so paths with spaces stay one argument.
fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':' | '@'))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---------------------------------------------------------------------
    // Asset resolution
    // ---------------------------------------------------------------------

    #[test]
    fn selects_the_asset_for_each_supported_os_and_arch() {
        assert_eq!(
            asset_name("linux", "x86_64", false).unwrap(),
            "opm-linux-x86_64"
        );
        assert_eq!(
            asset_name("linux", "aarch64", false).unwrap(),
            "opm-linux-aarch64"
        );
        assert_eq!(
            asset_name("macos", "x86_64", false).unwrap(),
            "opm-macos-x86_64"
        );
        assert_eq!(
            asset_name("macos", "aarch64", false).unwrap(),
            "opm-macos-aarch64"
        );
    }

    #[test]
    fn prefers_the_webui_asset_only_where_a_webui_build_exists() {
        assert_eq!(
            asset_name("linux", "x86_64", true).unwrap(),
            "opm-linux-x86_64-webui"
        );
        assert_eq!(
            asset_name("linux", "aarch64", true).unwrap(),
            "opm-linux-aarch64-webui"
        );
        // release.yml publishes no macOS webui asset
        assert_eq!(
            asset_name("macos", "aarch64", true).unwrap(),
            "opm-macos-aarch64"
        );
    }

    #[test]
    fn rejects_platforms_without_a_release_asset() {
        assert!(asset_name("linux", "riscv64", false).is_err());
        assert!(asset_name("freebsd", "x86_64", false).is_err());
    }

    // ---------------------------------------------------------------------
    // Binary magic
    // ---------------------------------------------------------------------

    fn elf_prefix() -> Vec<u8> {
        let mut bytes = vec![0x7f];
        bytes.extend_from_slice(b"ELF");
        bytes.extend_from_slice(&[2, 1, 1, 0, 0, 0, 0, 0]);
        bytes
    }

    fn mach_o_64_prefix() -> Vec<u8> {
        vec![0xcf, 0xfa, 0xed, 0xfe, 1, 0, 0, 1]
    }

    #[test]
    fn accepts_elf_and_mach_o_executables() {
        assert!(is_executable_image(&elf_prefix()));
        assert!(is_executable_image(&mach_o_64_prefix()));
    }

    #[test]
    fn rejects_an_html_error_page_and_an_empty_body() {
        let html = b"<!DOCTYPE html><html><body>404: Not Found</body></html>";
        assert!(!is_executable_image(html));
        assert!(!is_executable_image(&[]));
        assert!(!is_executable_image(b"\x7f"));
    }

    // ---------------------------------------------------------------------
    // Version comparison
    // ---------------------------------------------------------------------

    #[test]
    fn compares_versions_numerically_not_lexically() {
        assert!(version_key("2.0.0") > version_key("1.10.0"));
        assert!(version_key("2.0.10") > version_key("2.0.9"));
        assert!(version_key("2.0.0") < version_key("2.0.1"));
        assert!(version_key("2.0.0") == version_key("v2.0.0"));
    }

    #[test]
    fn a_build_of_the_same_version_is_not_something_to_install() {
        // Same semver, different git sha: the asset is still that version.
        assert!(!version_is_newer("2.0.0 (2026-1-1 abc) [release]", "2.0.0"));
        assert!(version_is_newer("2.0.0 (2026-1-1 abc) [release]", "2.0.1"));
        assert!(!version_is_newer("2.0.0", "2.0.0"));
    }

    fn release(body: &str) -> Release {
        Release {
            tag: "latest".to_string(),
            body: Some(body.to_string()),
        }
    }

    #[test]
    fn reads_the_commit_the_release_was_built_from() {
        let body = concat!(
            "## Latest Development Build\n\n",
            "**Commit:** 61e696e5d6287af2a8ed805450f8436fbb9e7785\n",
            "**Date:** 2026-02-26T11:37:34Z\n"
        );
        assert_eq!(
            commit_from_body(Some(body)).unwrap(),
            "61e696e5d6287af2a8ed805450f8436fbb9e7785"
        );
        assert_eq!(commit_from_body(Some("no identity here")), None);
        assert_eq!(commit_from_body(None), None);
        // the heading must not be mistaken for a commit
        assert_eq!(commit_from_body(Some("## Latest Development Build")), None);
    }

    #[test]
    fn the_release_commit_identifies_the_build() {
        let local = env!("GIT_HASH").trim().to_ascii_lowercase();
        let same = format!("## Latest Development Build\n\n**Commit:** {local}\n");
        let other = "## Latest Development Build\n\n**Commit:** 0123456789abcdef0123\n";

        // the published build is this very commit: nothing to install
        assert!(release(&same).is_this_build());
        // another commit is a different, installable build
        assert!(!release(other).is_this_build());
        // a release without a commit cannot be proven to be this build
        assert!(!release("## Latest Development Build").is_this_build());
    }

    // ---------------------------------------------------------------------
    // Repository argument
    // ---------------------------------------------------------------------

    #[test]
    fn accepts_an_owner_slash_name_override_and_rejects_anything_else() {
        assert_eq!(
            parse_repo("h0dev/opm").unwrap(),
            Repo {
                owner: "h0dev".to_string(),
                name: "opm".to_string()
            }
        );
        assert!(parse_repo("h0dev").is_err());
        assert!(parse_repo("/opm").is_err());
        assert!(parse_repo("h0dev/").is_err());
        assert!(parse_repo("a/b/c").is_err());
    }

    #[test]
    fn builds_the_release_asset_url() {
        assert_eq!(
            asset_url("h0dev/opm", "opm-linux-x86_64"),
            "https://github.com/h0dev/opm/releases/download/latest/opm-linux-x86_64"
        );
    }

    // ---------------------------------------------------------------------
    // Not-writable path decision
    // ---------------------------------------------------------------------

    #[test]
    fn a_writable_directory_can_be_replaced() {
        let dir = fresh_dir("writable");
        assert_eq!(
            decide(&dir.join("opm"), /* writable */ true),
            InstallDecision::Replace
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unwritable_directory_is_told_the_command_to_run_instead() {
        let dir = fresh_dir("unwritable");
        assert_eq!(
            decide(&dir.join("opm"), /* writable */ false),
            InstallDecision::NeedsPrivileges {
                command: "sudo opm update --yes".to_string()
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_target_from_a_writable_directory_is_still_a_replace() {
        let dir = fresh_dir("missing-target");
        assert_eq!(
            decide(&dir.join("opm"), true),
            InstallDecision::Replace
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn fresh_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "opm-update-test-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // ---------------------------------------------------------------------
    // Applying the download beside the target
    // ---------------------------------------------------------------------

    #[test]
    fn install_keeps_the_old_binary_and_swaps_in_the_new_one() {
        let dir = fresh_dir("install");
        let target = dir.join("opm");
        std::fs::write(&target, b"old build").unwrap();

        let new_bytes = elf_prefix();
        install_beside(&target, &new_bytes).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), new_bytes);
        assert_eq!(std::fs::read(dir.join("opm.old")).unwrap(), b"old build");
        // no partial temp file may survive a successful install
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(".opm.update."))
            .collect();
        assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "installed binary must be executable");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_refuses_a_body_that_is_not_an_executable() {
        let dir = fresh_dir("install-bad");
        let target = dir.join("opm");
        std::fs::write(&target, b"old build").unwrap();

        let err = install_beside(&target, b"<html>404</html>").unwrap_err();
        assert!(err.contains("not an executable"), "{err}");
        assert_eq!(std::fs::read(&target).unwrap(), b"old build");
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(".opm.update."))
            .collect();
        assert!(leftovers.is_empty(), "leftover temp files: {leftovers:?}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn directory_writability_is_measured_by_the_actual_probe() {
        let dir = fresh_dir("probe");
        assert!(directory_is_writable(&dir));
        let _ = std::fs::remove_dir_all(&dir);

        let gone = dir.join("does-not-exist");
        assert!(!directory_is_writable(&gone));
    }

    #[test]
    fn the_default_repo_is_the_one_named_in_help() {
        assert_eq!(DEFAULT_REPO, "h0dev/opm");
        assert!(parse_repo(DEFAULT_REPO).is_ok());
    }

    /// A privileged re-run must replace the binary that is running, not whatever
    /// `opm` happens to resolve to on PATH. Installed at ~/.local/bin or
    /// /usr/bin, the update stays there.
    #[test]
    fn self_invocation_names_this_binary_by_path() {
        let options = Options {
            check: false,
            yes: true,
            force: false,
            asset: None,
            repo: DEFAULT_REPO.to_string(),
        };

        let command = self_invocation(&options);
        let running = running_binary().expect("this test runs from a binary");
        let expected = shell_quote(&running.to_string_lossy());

        assert!(
            command.starts_with(&format!("{expected} update --yes")),
            "expected the running binary's own path, got: {command}"
        );
        assert!(
            running.is_absolute(),
            "the re-run needs an absolute path so PATH order cannot pick another binary"
        );
    }

    #[test]
    fn self_invocation_carries_options_and_quotes_them() {
        let options = Options {
            check: false,
            yes: true,
            force: true,
            asset: Some("my asset".to_string()),
            repo: "owner/repo".to_string(),
        };

        let command = self_invocation(&options);
        assert!(command.contains("--force"), "got: {command}");
        assert!(
            command.contains("--asset 'my asset'"),
            "a value with a space must stay one argument, got: {command}"
        );
        assert!(command.contains("--repo owner/repo"), "got: {command}");
    }

    #[test]
    fn shell_quote_leaves_plain_paths_alone_and_escapes_the_rest() {
        assert_eq!(shell_quote("/usr/local/bin/opm"), "/usr/local/bin/opm");
        assert_eq!(shell_quote("/home/u/.local/bin/opm"), "/home/u/.local/bin/opm");
        assert_eq!(shell_quote("/tmp/a b/opm"), "'/tmp/a b/opm'");
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }
}
