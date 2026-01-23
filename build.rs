use chrono::Datelike;
use flate2::read::GzDecoder;
use reqwest;
use tar::Archive;

use std::{
    env,
    fs::{self, File},
    io::{self, copy},
    path::{Path, PathBuf},
    process::Command,
};

const NODE_VERSION: &str = "20.11.0";

fn extract_tar_gz(tar: &PathBuf, download_dir: &PathBuf) -> io::Result<()> {
    let file = File::open(tar)?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    archive.unpack(download_dir)?;
    Ok(fs::remove_file(tar)?)
}

fn download_file(url: String, destination: &PathBuf, download_dir: &PathBuf) {
    if !download_dir.exists() {
        fs::create_dir_all(download_dir).unwrap();
    }

    let mut response = reqwest::blocking::get(url).expect("Failed to send request");
    let mut file = File::create(destination).expect("Failed to create file");

    copy(&mut response, &mut file).expect("Failed to copy content");
}

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

fn download_node() -> PathBuf {
    #[cfg(target_os = "linux")]
    let target_os = "linux";
    #[cfg(all(target_os = "macos"))]
    let target_os = "darwin";

    #[cfg(all(target_arch = "arm"))]
    let target_arch = "armv7l";
    #[cfg(all(target_arch = "x86_64"))]
    let target_arch = "x64";
    #[cfg(all(target_arch = "aarch64"))]
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
    let node_archive = download_dir.join(format!("node-v{}-{}.tar.gz", NODE_VERSION, target_os));
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
    if npm.extension().and_then(|s| s.to_str()) == Some("js") {
        // Downloaded npm - run as script
        Command::new(node)
            .args([npm.to_str().unwrap(), "ci"])
            .current_dir(project_dir)
            .env("PATH", &path)
            .status()
            .expect("Failed to install dependencies");
    } else {
        // System npm - run as binary
        Command::new(&npm)
            .args(["ci"])
            .current_dir(project_dir)
            .env("PATH", &path)
            .status()
            .expect("Failed to install dependencies");
    }

    /* build frontend */
    Command::new(node)
        .args(["node_modules/astro/astro.js", "build"])
        .current_dir(project_dir)
        .env("PATH", &path)
        .status()
        .expect("Failed to build frontend");
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
        .args(&["rev-parse", "--short=10", "HEAD"])
        .output()
        .unwrap();
    let output_full = Command::new("git")
        .args(&["rev-parse", "HEAD"])
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
        "debug" => {
            println!("cargo:rustc-env=PROFILE=debug");
            
            /* create dist directory and placeholder HTML files for debug builds */
            let dist_dir = Path::new("src/webui/dist");
            fs::create_dir_all(dist_dir).expect("Failed to create dist directory");
            
            let placeholder = "<html><body><h1>Debug Mode - WebUI not built</h1></body></html>";
            let html_files = vec![
                "view.html",
                "login.html",
                "index.html",
                "status.html",
                "servers.html",
                "system.html",
                "events.html",
                "agent-detail.html",
            ];
            
            for file in html_files {
                let file_path = dist_dir.join(file);
                if !file_path.exists() {
                    fs::write(file_path, placeholder).expect("Failed to create placeholder HTML file");
                }
            }
        }
        "release" => {
            println!("cargo:rustc-env=PROFILE=release");

            /* cleanup */
            fs::remove_dir_all(format!("src/webui/dist")).ok();

            /* pre-build */
            let node_bin_dir = use_system_node_or_download();
            download_then_build(node_bin_dir);
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
