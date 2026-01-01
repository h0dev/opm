use chrono::Datelike;

use std::{
    env,
    process::Command,
};

fn main() {
    #[cfg(target_os = "windows")]
    compile_error!("This project is not supported on Windows.");

    #[cfg(target_arch = "x86")]
    compile_error!("This project is not supported on 32 bit.");

    /* version attributes */
    let date = chrono::Utc::now();
    let profile = env::var("PROFILE").unwrap();
    let output = Command::new("git").args(&["rev-parse", "--short=10", "HEAD"]).output().unwrap();
    let output_full = Command::new("git").args(&["rev-parse", "HEAD"]).output().unwrap();

    println!("cargo:rustc-env=TARGET={}", env::var("TARGET").unwrap());
    println!("cargo:rustc-env=GIT_HASH={}", String::from_utf8(output.stdout).unwrap());
    println!("cargo:rustc-env=GIT_HASH_FULL={}", String::from_utf8(output_full.stdout).unwrap());
    println!("cargo:rustc-env=BUILD_DATE={}-{}-{}", date.year(), date.month(), date.day());

    /* profile matching */
    match profile.as_str() {
        "debug" => println!("cargo:rustc-env=PROFILE=debug"),
        "release" => println!("cargo:rustc-env=PROFILE=release"),
        _ => println!("cargo:rustc-env=PROFILE=none"),
    }

    let watched = vec![
        "lib",
        "src/lib.rs",
        "lib/include",
    ];

    watched.iter().for_each(|file| println!("cargo:rerun-if-changed={file}"));
}
