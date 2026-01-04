mod args;
pub use args::*;

pub(crate) mod import;
pub(crate) mod internal;

use internal::{Internal, STATS_PRE_LIST_DELAY_MS};
use macros_rs::{crashln, string, ternary};
use opm::{helpers, process::Runner};
use std::env;
use std::thread;
use std::time::Duration;

pub(crate) fn format(server_name: &String) -> (String, String) {
    let kind = ternary!(matches!(&**server_name, "internal" | "local"), "", "remote ").to_string();
    return (kind, server_name.to_string());
}

pub fn get_version(short: bool) -> String {
    return match short {
        true => format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        false => match env!("GIT_HASH") {
            "" => format!("{} ({}) [{}]", env!("CARGO_PKG_VERSION"), env!("BUILD_DATE"), env!("PROFILE")),
            hash => format!("{} ({} {hash}) [{}]", env!("CARGO_PKG_VERSION"), env!("BUILD_DATE"), env!("PROFILE")),
        },
    };
}

pub fn start(name: &Option<String>, args: &Args, watch: &Option<String>, max_memory: &Option<String>, reset_env: &bool, server_name: &String) {
    let mut runner = Runner::new();
    let (kind, list_name) = format(server_name);

    let arg = match args.get_string() {
        Some(arg) => arg,
        None => "",
    };

    if arg == "all" {
        println!("{} Applying {kind}action startAllProcess", *helpers::SUCCESS);

        let largest = runner.size();
        match largest {
            Some(largest) => (0..*largest + 1).for_each(|id| {
                runner = Internal {
                    id,
                    server_name,
                    kind: kind.clone(),
                    runner: runner.clone(),
                }
                .restart(name, watch, *reset_env, true);
            }),
            None => println!("{} Cannot start all, no processes found", *helpers::FAIL),
        }
    } else {
        match args {
            Args::Id(id) => {
                Internal { id: *id, runner, server_name, kind }.restart(name, watch, *reset_env, false);
            }
            Args::Script(script) => match runner.find(&script, server_name) {
                Some(id) => {
                    Internal { id, runner, server_name, kind }.restart(name, watch, *reset_env, false);
                }
                None => {
                    Internal { id: 0, runner, server_name, kind }.create(script, name, watch, max_memory, false);
                }
            },
        }
    }

    // Allow CPU stats to accumulate before displaying the list
    thread::sleep(Duration::from_millis(STATS_PRE_LIST_DELAY_MS));
    Internal::list(&string!("default"), &list_name);
}

pub fn stop(items: &Items, server_name: &String) {
    let mut runner: Runner = Runner::new();
    let (kind, list_name) = format(server_name);

    if items.is_all() {
        println!("{} Applying {kind}action stopAllProcess", *helpers::SUCCESS);

        let largest = runner.size();
        match largest {
            Some(largest) => (0..*largest + 1).for_each(|id| {
                runner = Internal {
                    id,
                    server_name,
                    kind: kind.clone(),
                    runner: runner.clone(),
                }
                .stop(true);
            }),
            None => println!("{} Cannot stop all, no processes found", *helpers::FAIL),
        }
    } else {
        for item in &items.items {
            match item {
                Item::Id(id) => {
                    runner = Internal {
                        id: *id,
                        server_name,
                        kind: kind.clone(),
                        runner: runner.clone(),
                    }
                    .stop(false);
                }
                Item::Name(name) => match runner.find(&name, server_name) {
                    Some(id) => {
                        runner = Internal {
                            id,
                            server_name,
                            kind: kind.clone(),
                            runner: runner.clone(),
                        }
                        .stop(false);
                    }
                    None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
                },
            }
        }
    }

    Internal::list(&string!("default"), &list_name);
}

pub fn remove(items: &Items, server_name: &String) {
    let runner: Runner = Runner::new();
    let (kind, _) = format(server_name);

    for item in &items.items {
        match item {
            Item::Id(id) => {
                Internal {
                    id: *id,
                    runner: runner.clone(),
                    server_name,
                    kind: kind.clone(),
                }
                .remove()
            }
            Item::Name(name) => match runner.find(&name, server_name) {
                Some(id) => {
                    Internal {
                        id,
                        runner: runner.clone(),
                        server_name,
                        kind: kind.clone(),
                    }
                    .remove()
                }
                None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
            },
        }
    }

    super::daemon::reset();
}

pub fn info(item: &Item, format: &String, server_name: &String) {
    let runner: Runner = Runner::new();
    let (kind, _) = self::format(server_name);

    match item {
        Item::Id(id) => Internal { id: *id, runner, server_name, kind }.info(format),
        Item::Name(name) => match runner.find(&name, server_name) {
            Some(id) => Internal { id, runner, server_name, kind }.info(format),
            None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
        },
    }
}

pub fn logs(item: &Item, lines: &usize, server_name: &String, follow: bool, filter: Option<&str>, errors_only: bool, stats: bool) {
    let runner: Runner = Runner::new();
    let (kind, _) = format(server_name);

    match item {
        Item::Id(id) => Internal { id: *id, runner, server_name, kind }.logs(lines, follow, filter, errors_only, stats),
        Item::Name(name) => match runner.find(&name, server_name) {
            Some(id) => Internal { id, runner, server_name, kind }.logs(lines, follow, filter, errors_only, stats),
            None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
        },
    }
}

// combine into a single function that handles multiple
pub fn env(item: &Item, server_name: &String) {
    let runner: Runner = Runner::new();
    let (kind, _) = format(server_name);

    match item {
        Item::Id(id) => Internal { id: *id, runner, server_name, kind }.env(),
        Item::Name(name) => match runner.find(&name, server_name) {
            Some(id) => Internal { id, runner, server_name, kind }.env(),
            None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
        },
    }
}

pub fn flush(item: &Item, server_name: &String) {
    let runner: Runner = Runner::new();
    let (kind, _) = format(server_name);

    match item {
        Item::Id(id) => Internal { id: *id, runner, server_name, kind }.flush(),
        Item::Name(name) => match runner.find(&name, server_name) {
            Some(id) => Internal { id, runner, server_name, kind }.flush(),
            None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
        },
    }
}

pub fn restart(items: &Items, server_name: &String) {
    let mut runner: Runner = Runner::new();
    let (kind, list_name) = format(server_name);

    if items.is_all() {
        println!("{} Applying {kind}action restartAllProcess", *helpers::SUCCESS);

        let largest = runner.size();
        match largest {
            Some(largest) => (0..*largest + 1).for_each(|id| {
                runner = Internal {
                    id,
                    server_name,
                    kind: kind.clone(),
                    runner: runner.clone(),
                }
                .restart(&None, &None, false, true);
            }),
            None => println!("{} Cannot restart all, no processes found", *helpers::FAIL),
        }
    } else {
        for item in &items.items {
            match item {
                Item::Id(id) => {
                    runner = Internal {
                        id: *id,
                        server_name,
                        kind: kind.clone(),
                        runner: runner.clone(),
                    }
                    .restart(&None, &None, false, false);
                }
                Item::Name(name) => match runner.find(&name, server_name) {
                    Some(id) => {
                        runner = Internal {
                            id,
                            server_name,
                            kind: kind.clone(),
                            runner: runner.clone(),
                        }
                        .restart(&None, &None, false, false);
                    }
                    None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
                },
            }
        }
    }

    // Allow CPU stats to accumulate before displaying the list
    thread::sleep(Duration::from_millis(STATS_PRE_LIST_DELAY_MS));
    Internal::list(&string!("default"), &list_name);
}

pub fn get_command(item: &Item, server_name: &String) {
    let runner: Runner = Runner::new();
    let (kind, _) = format(server_name);

    match item {
        Item::Id(id) => Internal { id: *id, runner, server_name, kind }.get_command(),
        Item::Name(name) => match runner.find(&name, server_name) {
            Some(id) => Internal { id, runner, server_name, kind }.get_command(),
            None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
        },
    }
}
