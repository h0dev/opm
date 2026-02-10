use chrono::Datelike;

#[cfg(feature = "webui")]
use flate2::read::GzDecoder;
#[cfg(feature = "webui")]
use reqwest;
#[cfg(feature = "webui")]
use tar::Archive;

use std::{env, process::Command};

#[cfg(feature = "webui")]
use std::{
    fs::{self, File},
    io::{self, copy},
    path::{Path, PathBuf},
};

#[cfg(feature = "webui")]
const NODE_VERSION: &str = "20.11.0";

#[cfg(feature = "webui")]
fn extract_tar_gz(tar: &PathBuf, download_dir: &PathBuf) -> io::Result<()> {
    let file = File::open(tar)?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    archive.unpack(download_dir)?;
    Ok(fs::remove_file(tar)?)
}

#[cfg(feature = "webui")]
fn download_file(url: String, destination: &PathBuf, download_dir: &PathBuf) {
    if !download_dir.exists() {
        fs::create_dir_all(download_dir).unwrap();
    }

    let mut response = reqwest::blocking::get(url).expect("Failed to send request");
    let mut file = File::create(destination).expect("Failed to create file");

    copy(&mut response, &mut file).expect("Failed to copy content");
}

#[cfg(feature = "webui")]
fn use_system_node_or_download() -> PathBuf {
    // Try to use system Node.js first
    if let Ok(node_path) = Command::new("which").arg("node").output() {
        if node_path.status.success() {
            let node_bin = String::from_utf8_lossy(&node_path.stdout)
                .trim()
                .to_string();
            if !node_bin.is_empty() {
                // Get the bin directory containing node
                let node_path = PathBuf::from(node_bin);
                if let Some(bin_dir) = node_path.parent() {
                    eprintln!("Using system Node.js from {:?}", bin_dir);
                    return bin_dir.to_path_buf();
                }
            }
        }
    }

    // Fall back to downloading Node.js
    eprintln!("System Node.js not found, downloading...");
    download_node()
}

#[cfg(feature = "webui")]
fn download_node() -> PathBuf {
    #[cfg(target_os = "linux")]
    let target_os = "linux";
    #[cfg(target_os = "macos")]
    let target_os = "darwin";

    #[cfg(target_arch = "arm")]
    let target_arch = "armv7l";
    #[cfg(target_arch = "x86_64")]
    let target_arch = "x64";
    #[cfg(target_arch = "aarch64")]
    let target_arch = "arm64";

    let download_url = format!(
        "https://nodejs.org/dist/v{NODE_VERSION}/node-v{NODE_VERSION}-{target_os}-{target_arch}.tar.gz"
    );

    /* paths */
    let download_dir = Path::new("target").join("downloads");
    let node_extract_dir =
        download_dir.join(format!("node-v{NODE_VERSION}-{target_os}-{target_arch}"));

    if node_extract_dir.is_dir() {
        return node_extract_dir;
    }

    /* download node */
    let node_archive = download_dir.join(format!(
        "node-v{NODE_VERSION}-{target_os}-{target_arch}.tar.gz"
    ));
    download_file(download_url, &node_archive, &download_dir);

    /* extract node */
    if let Err(err) = extract_tar_gz(&node_archive, &download_dir) {
        panic!("Failed to extract Node.js: {:?}", err)
    }

    println!(
        "cargo:rustc-env=NODE_HOME={}",
        node_extract_dir.to_str().unwrap()
    );

    return node_extract_dir;
}

#[cfg(feature = "webui")]
fn download_then_build(node_bin_dir: PathBuf) {
    let bin = &node_bin_dir;
    let node = &bin.join("node");
    let project_dir = &Path::new("src").join("webui");

    // Check if this is system Node or downloaded Node
    let npm = if bin.join("npm").exists() {
        // System Node with npm binary
        bin.join("npm")
    } else {
        // Downloaded Node with npm as a script
        let parent = node_bin_dir
            .parent()
            .expect("Node binary directory should have a parent directory");
        parent.join("lib/node_modules/npm/index.js")
    };

    /* set path */
    let mut paths = match env::var_os("PATH") {
        Some(paths) => env::split_paths(&paths).collect::<Vec<PathBuf>>(),
        None => vec![],
    };

    paths.push(bin.clone());

    let path = match env::join_paths(paths) {
        Ok(joined) => joined,
        Err(err) => panic!("{err}"),
    };

    /* install deps */
    let npm_status = if npm.extension().and_then(|s| s.to_str()) == Some("js") {
        // Downloaded npm - run as script
        Command::new(node)
            .args([npm.to_str().unwrap(), "ci"])
            .current_dir(project_dir)
            .env("PATH", &path)
            .status()
            .expect("Failed to start npm ci command")
    } else {
        // System npm - run as binary
        Command::new(&npm)
            .args(["ci"])
            .current_dir(project_dir)
            .env("PATH", &path)
            .status()
            .expect("Failed to start npm ci command")
    };

    if !npm_status.success() {
        panic!(
            "npm ci command failed with exit code: {:?}\n\
             \n\
             This error occurs when building with the 'webui' feature.\n\
             Possible solutions:\n\
             - Check that Node.js and npm are properly installed\n\
             - Try running 'npm install' manually in the src/webui directory\n\
             - Build without the webui feature: cargo build --release",
            npm_status.code()
        );
    }

    /* build frontend */
    let build_status = Command::new(node)
        .args(["node_modules/astro/astro.js", "build"])
        .current_dir(project_dir)
        .env("PATH", &path)
        .status()
        .expect("Failed to start astro build command");

    if !build_status.success() {
        panic!(
            "Astro build command failed with exit code: {:?}\n\
             \n\
             This error occurs when building the web UI frontend.\n\
             Possible solutions:\n\
             - Check the error messages above for specific build failures\n\
             - Try running 'npm run build' manually in the src/webui directory\n\
             - Build without the webui feature: cargo build --release",
            build_status.code()
        );
    }
}

fn main() {
    #[cfg(target_os = "windows")]
    compile_error!("This project is not supported on Windows.");

    #[cfg(target_arch = "x86")]
    compile_error!("This project is not supported on 32 bit.");

    /* version attributes */
    let date = chrono::Utc::now();
    let profile = env::var("PROFILE").unwrap();
    let output = Command::new("git")
        .args(["rev-parse", "--short=10", "HEAD"])
        .output()
        .unwrap();
    let output_full = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();

    println!("cargo:rustc-env=TARGET={}", env::var("TARGET").unwrap());
    println!(
        "cargo:rustc-env=GIT_HASH={}",
        String::from_utf8(output.stdout).unwrap()
    );
    println!(
        "cargo:rustc-env=GIT_HASH_FULL={}",
        String::from_utf8(output_full.stdout).unwrap()
    );
    println!(
        "cargo:rustc-env=BUILD_DATE={}-{}-{}",
        date.year(),
        date.month(),
        date.day()
    );

    /* profile matching */
    match profile.as_str() {
        "debug" => println!("cargo:rustc-env=PROFILE=debug"),
        "release" => {
            println!("cargo:rustc-env=PROFILE=release");

            // Only build webui if the feature is enabled
            #[cfg(feature = "webui")]
            {
                /* cleanup */
                fs::remove_dir_all("src/webui/dist").ok();

                /* pre-build */
                let node_bin_dir = use_system_node_or_download();
                download_then_build(node_bin_dir);
            }
        }
        _ => println!("cargo:rustc-env=PROFILE=none"),
    }

    let watched = vec![
        "lib",
        "src/lib.rs",
        "lib/include",
        "src/webui/src",
        "src/webui/links.ts",
        "src/webui/package.json",
        "src/webui/tsconfig.json",
        "src/webui/astro.config.mjs",
        "src/webui/tailwind.config.mjs",
    ];

    watched
        .iter()
        .for_each(|file| println!("cargo:rerun-if-changed={file}"));
}
