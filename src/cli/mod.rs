mod args;
pub use args::*;

pub(crate) mod events;
pub(crate) mod import;
pub(crate) mod internal;

use internal::{Internal, STATS_PRE_LIST_DELAY_MS};
use macros_rs::{crashln, string, ternary};
use opm::{
    config, helpers,
    process::{Identity, Runner},
};
use std::env;
use std::thread;
use std::time::Duration;

// Local server identifiers
const LOCAL_SERVER_NAMES: [&str; 2] = ["internal", "local"];

pub(crate) fn format(server_name: &String) -> (String, String) {
    let kind = ternary!(
        LOCAL_SERVER_NAMES.contains(&server_name.as_str()),
        "",
        "remote "
    )
    .to_string();
    return (kind, server_name.to_string());
}

/// Check if the current role allows remote operations
pub(crate) fn check_remote_permission(server_name: &String) {
    let config = config::read();

    // If trying to access a remote server and role is agent, deny
    if config.is_agent() && !LOCAL_SERVER_NAMES.contains(&server_name.as_str()) {
        crashln!(
            "{} Agent role cannot perform remote operations. Only local process management is allowed.",
            *helpers::FAIL
        );
    }
}

pub fn get_version(short: bool) -> String {
    return match short {
        true => format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        false => match env!("GIT_HASH") {
            "" => format!(
                "{} ({}) [{}]",
                env!("CARGO_PKG_VERSION"),
                env!("BUILD_DATE"),
                env!("PROFILE")
            ),
            hash => format!(
                "{} ({} {hash}) [{}]",
                env!("CARGO_PKG_VERSION"),
                env!("BUILD_DATE"),
                env!("PROFILE")
            ),
        },
    };
}

pub fn start(
    name: &Option<String>,
    args: &Args,
    watch: &Option<String>,
    max_memory: &Option<String>,
    reset_env: &bool,
    server_name: &String,
    workers: &Option<usize>,
    port_range: &Option<String>,
) {
    // Check permissions for remote operations
    check_remote_permission(server_name);

    let mut runner = Runner::new();
    let (kind, list_name) = format(server_name);

    let arg = match args.get_string() {
        Some(arg) => arg,
        None => "",
    };

    // Handle worker load balancing
    if let Some(worker_count) = workers {
        if *worker_count < 2 {
            crashln!(
                "{} Worker count must be at least 2 for load balancing",
                *helpers::FAIL
            );
        }

        // Parse port range if provided
        let ports = if let Some(port_str) = port_range {
            parse_port_range(port_str)
        } else {
            vec![]
        };

        // Validate port range matches worker count if ports are specified
        if !ports.is_empty() && ports.len() != *worker_count {
            crashln!(
                "{} Port range must provide exactly {} ports for {} workers",
                *helpers::FAIL,
                worker_count,
                worker_count
            );
        }

        // Start multiple worker instances
        println!(
            "{} Starting {} worker instances for load balancing",
            *helpers::SUCCESS,
            worker_count
        );

        for i in 0..*worker_count {
            let worker_name = if let Some(base_name) = name {
                Some(format!("{}-worker-{}", base_name, i + 1))
            } else {
                Some(format!("worker-{}", i + 1))
            };

            // Determine port info for display
            let port_info = if !ports.is_empty() {
                format!(" (PORT={})", ports[i])
            } else if let Some(port_str) = port_range {
                format!(" (PORT={} via SO_REUSEPORT)", port_str)
            } else {
                String::new()
            };

            println!(
                "  {} Starting worker {} of {}{}",
                *helpers::SUCCESS,
                i + 1,
                worker_count,
                port_info
            );

            // Create each worker as a new process
            runner = Internal {
                id: 0, // 0 means create new process
                server_name,
                kind: kind.clone(),
                runner: runner.clone(),
            }
            .create(&arg.to_string(), &worker_name, watch, &None, true);
        }

        runner.save();

        println!(
            "{} All {} workers started successfully",
            *helpers::SUCCESS,
            worker_count
        );

        // Allow CPU stats to accumulate before displaying the list
        thread::sleep(Duration::from_millis(STATS_PRE_LIST_DELAY_MS));
        Internal::list_with_runner(&string!("default"), &list_name, Some(&runner));
        return;
    }

    if arg == "all" {
        println!(
            "{} Applying {kind}action startAllProcess",
            *helpers::SUCCESS
        );

        let process_ids: Vec<usize> = runner.items().keys().copied().collect();

        if process_ids.is_empty() {
            println!("{} Cannot start all, no processes found", *helpers::FAIL);
        } else {
            for id in process_ids {
                runner = Internal {
                    id,
                    server_name,
                    kind: kind.clone(),
                    runner: runner.clone(),
                }
                .restart(name, watch, *reset_env, true, false); // start all - don't increment
            }
            runner.save();
        }

        // Allow CPU stats to accumulate before displaying the list
        thread::sleep(Duration::from_millis(STATS_PRE_LIST_DELAY_MS));
        Internal::list_with_runner(&string!("default"), &list_name, Some(&runner));
        return;
    } else {
        match args {
            Args::Id(id) => {
                runner = Internal {
                    id: *id,
                    runner,
                    server_name,
                    kind,
                }
                .restart(name, watch, *reset_env, false, false); // start by id - don't increment
                runner.save();
            }
            Args::Script(script) => match runner.find(&script, server_name) {
                Some(id) => {
                    runner = Internal {
                        id,
                        runner,
                        server_name,
                        kind,
                    }
                    .restart(name, watch, *reset_env, false, false); // start existing - don't increment
                    runner.save();
                }
                None => {
                    runner = Internal {
                        id: 0,
                        runner,
                        server_name,
                        kind,
                    }
                    .create(script, name, watch, max_memory, false);
                    runner.save();
                }
            },
        }
    }

    // Allow CPU stats to accumulate before displaying the list
    thread::sleep(Duration::from_millis(STATS_PRE_LIST_DELAY_MS));
    Internal::list_with_runner(&string!("default"), &list_name, Some(&runner));
}

fn parse_port_range(port_str: &str) -> Vec<u16> {
    if port_str.contains('-') {
        // Parse range like "3000-3010"
        let parts: Vec<&str> = port_str.split('-').collect();
        if parts.len() != 2 {
            crashln!(
                "{} Invalid port range format. Use 'start-end' (e.g., 3000-3010)",
                *helpers::FAIL
            );
        }

        let start: u16 = parts[0]
            .parse()
            .unwrap_or_else(|_| crashln!("{} Invalid start port number", *helpers::FAIL));
        let end: u16 = parts[1]
            .parse()
            .unwrap_or_else(|_| crashln!("{} Invalid end port number", *helpers::FAIL));

        if start >= end {
            crashln!("{} Start port must be less than end port", *helpers::FAIL);
        }

        (start..=end).collect()
    } else {
        // Single port - return empty vec to signal SO_REUSEPORT mode
        vec![]
    }
}

pub fn stop(items: &Items, server_name: &String) {
    // Check permissions for remote operations
    check_remote_permission(server_name);

    let mut runner: Runner = Runner::new();
    let (kind, list_name) = format(server_name);

    if items.is_all() {
        println!("{} Applying {kind}action stopAllProcess", *helpers::SUCCESS);

        let process_ids: Vec<usize> = runner.items().keys().copied().collect();

        if process_ids.is_empty() {
            println!("{} Cannot stop all, no processes found", *helpers::FAIL);
        } else {
            for id in process_ids {
                runner = Internal {
                    id,
                    server_name,
                    kind: kind.clone(),
                    runner: runner.clone(),
                }
                .stop(true);
            }
            runner.save();
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
        runner.save();
    }

    Internal::list(&string!("default"), &list_name);
}

pub fn remove(items: &Items, server_name: &String) {
    // Check permissions for remote operations
    check_remote_permission(server_name);

    let runner: Runner = Runner::new();
    let (kind, _) = format(server_name);

    if items.is_all() {
        println!(
            "{} Applying {kind}action removeAllProcess",
            *helpers::SUCCESS
        );

        if runner.items().is_empty() {
            println!("{} Cannot remove all, no processes found", *helpers::FAIL);
            return;
        }

        let requested: Vec<Item> = runner.items().keys().map(|id| Item::Id(*id)).collect();
        remove_targets_by_identity(
            resolve_removal_targets(&requested, &runner, server_name),
            server_name,
            &kind,
        );
    } else {
        remove_targets_by_identity(
            resolve_removal_targets(&items.items, &runner, server_name),
            server_name,
            &kind,
        );
    }

    // NOTE: Removed daemon::reset() call here to fix Bug #2
    // Deletions should only be persisted to disk when user explicitly runs 'opm save'
    // The reset() function was directly writing to permanent storage (process.dump)
    // which violated the memory-first architecture where changes stay in RAM until saved
}

/// Remove the resolved targets one at a time.
///
/// The identity resolution in [`remove_targets`] is what keeps a batch safe: each
/// removal compacts the ids, so an id from the batch snapshot must never be
/// reused to address the list again. A remote server cannot be addressed by
/// identity, so there the id the target had when the batch was resolved is used.
fn remove_targets_by_identity(targets: Vec<RemovalTarget>, server_name: &String, kind: &str) {
    let local = LOCAL_SERVER_NAMES.contains(&server_name.as_str());

    remove_targets(
        targets,
        kind,
        || if local { Runner::new() } else { Runner::default() },
        |id| {
            Internal {
                id,
                server_name,
                kind: kind.to_string(),
                runner: Runner::default(),
            }
            .remove();
            Ok(())
        },
    );
}

pub fn info(item: &Item, format: &String, server_name: &String) {
    // Check permissions for remote operations
    check_remote_permission(server_name);

    let runner: Runner = Runner::new();
    let (kind, _) = self::format(server_name);

    match item {
        Item::Id(id) => Internal {
            id: *id,
            runner,
            server_name,
            kind,
        }
        .info(format),
        Item::Name(name) => match runner.find(&name, server_name) {
            Some(id) => Internal {
                id,
                runner,
                server_name,
                kind,
            }
            .info(format),
            None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
        },
    }
}

pub fn logs(
    item: &Item,
    lines: &usize,
    server_name: &String,
    follow: bool,
    filter: Option<&str>,
    errors_only: bool,
    stats: bool,
) {
    // Check permissions for remote operations
    check_remote_permission(server_name);

    let runner: Runner = Runner::new();
    let (kind, _) = format(server_name);

    match item {
        Item::Id(id) => Internal {
            id: *id,
            runner,
            server_name,
            kind,
        }
        .logs(lines, follow, filter, errors_only, stats),
        Item::Name(name) => match runner.find(&name, server_name) {
            Some(id) => Internal {
                id,
                runner,
                server_name,
                kind,
            }
            .logs(lines, follow, filter, errors_only, stats),
            None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
        },
    }
}

// combine into a single function that handles multiple
pub fn env(item: &Item, server_name: &String) {
    // Check permissions for remote operations
    check_remote_permission(server_name);

    let runner: Runner = Runner::new();
    let (kind, _) = format(server_name);

    match item {
        Item::Id(id) => Internal {
            id: *id,
            runner,
            server_name,
            kind,
        }
        .env(),
        Item::Name(name) => match runner.find(&name, server_name) {
            Some(id) => Internal {
                id,
                runner,
                server_name,
                kind,
            }
            .env(),
            None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
        },
    }
}

pub fn flush(item: &Item, server_name: &String) {
    // Check permissions for remote operations
    check_remote_permission(server_name);

    let runner: Runner = Runner::new();
    let (kind, _) = format(server_name);

    match item {
        Item::Id(id) => Internal {
            id: *id,
            runner,
            server_name,
            kind,
        }
        .flush(),
        Item::Name(name) => match runner.find(&name, server_name) {
            Some(id) => Internal {
                id,
                runner,
                server_name,
                kind,
            }
            .flush(),
            None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
        },
    }
}

pub fn restart(items: &Items, server_name: &String) {
    // Check permissions for remote operations
    check_remote_permission(server_name);

    let mut runner: Runner = Runner::new();
    let (kind, list_name) = format(server_name);

    if items.is_all() {
        println!(
            "{} Applying {kind}action restartAllProcess",
            *helpers::SUCCESS
        );

        let process_ids: Vec<usize> = runner.items().keys().copied().collect();

        if process_ids.is_empty() {
            println!("{} Cannot restart all, no processes found", *helpers::FAIL);
        } else {
            for id in process_ids {
                runner = Internal {
                    id,
                    server_name,
                    kind: kind.clone(),
                    runner: runner.clone(),
                }
                .restart(&None, &None, false, true, true); // restart all - increment counter
            }
            runner.save();
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
                    .restart(&None, &None, false, false, true); // restart by id - increment counter
                }
                Item::Name(name) => match runner.find(&name, server_name) {
                    Some(id) => {
                        runner = Internal {
                            id,
                            server_name,
                            kind: kind.clone(),
                            runner: runner.clone(),
                        }
                        .restart(&None, &None, false, false, true); // restart by name - increment counter
                    }
                    None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
                },
            }
        }
        runner.save();
    }

    // Allow CPU stats to accumulate before displaying the list
    thread::sleep(Duration::from_millis(STATS_PRE_LIST_DELAY_MS));
    Internal::list_with_runner(&string!("default"), &list_name, Some(&runner));
}

pub fn reload(items: &Items, server_name: &String) {
    // Check permissions for remote operations
    check_remote_permission(server_name);

    let mut runner: Runner = Runner::new();
    let (kind, list_name) = format(server_name);

    if items.is_all() {
        println!(
            "{} Applying {kind}action reloadAllProcess",
            *helpers::SUCCESS
        );

        let process_ids: Vec<usize> = runner.items().keys().copied().collect();

        if process_ids.is_empty() {
            println!("{} Cannot reload all, no processes found", *helpers::FAIL);
        } else {
            for id in process_ids {
                runner = Internal {
                    id,
                    server_name,
                    kind: kind.clone(),
                    runner: runner.clone(),
                }
                .reload(true);
            }
            runner.save();
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
                    .reload(false);
                }
                Item::Name(name) => match runner.find(&name, server_name) {
                    Some(id) => {
                        runner = Internal {
                            id,
                            server_name,
                            kind: kind.clone(),
                            runner: runner.clone(),
                        }
                        .reload(false);
                    }
                    None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
                },
            }
        }
        runner.save();
    }

    // Allow CPU stats to accumulate before displaying the list
    thread::sleep(Duration::from_millis(STATS_PRE_LIST_DELAY_MS));
    Internal::list_with_runner(&string!("default"), &list_name, Some(&runner));
}

pub fn get_command(item: &Item, server_name: &String) {
    // Check permissions for remote operations
    check_remote_permission(server_name);

    let runner: Runner = Runner::new();
    let (kind, _) = format(server_name);

    match item {
        Item::Id(id) => Internal {
            id: *id,
            runner,
            server_name,
            kind,
        }
        .get_command(),
        Item::Name(name) => match runner.find(&name, server_name) {
            Some(id) => Internal {
                id,
                runner,
                server_name,
                kind,
            }
            .get_command(),
            None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
        },
    }
}

pub fn adjust(item: &Item, command: &Option<String>, name: &Option<String>, server_name: &String) {
    // Check permissions for remote operations
    check_remote_permission(server_name);

    let runner: Runner = Runner::new();
    let (kind, _) = format(server_name);

    match item {
        Item::Id(id) => Internal {
            id: *id,
            runner,
            server_name,
            kind,
        }
        .adjust(command, name),
        Item::Name(item_name) => match runner.find(&item_name, server_name) {
            Some(id) => Internal {
                id,
                runner,
                server_name,
                kind,
            }
            .adjust(command, name),
            None => crashln!("{} Process ({item_name}) not found", *helpers::FAIL),
        },
    }
}

/// A process picked for deletion, identified by what the user picks it by rather
/// than by its id: compaction renumbers ids, so an id from a snapshot taken
/// earlier can address a different process by the time it is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemovalTarget {
    identity: Identity,
    /// The id it had when picked, used only to address a remote server.
    id: usize,
}

/// Resolve the requested items to the identity of the process each one names.
pub(crate) fn resolve_removal_targets(
    requested: &[Item],
    runner: &Runner,
    server_name: &String,
) -> Vec<RemovalTarget> {
    requested
        .iter()
        .map(|item| {
            let id = match item {
                Item::Id(id) => *id,
                Item::Name(name) => match runner.find(name, server_name) {
                    Some(id) => id,
                    None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
                },
            };

            let Some(process) = runner.info(id) else {
                crashln!("{} Process (id={}) not found", *helpers::FAIL, id);
            };

            RemovalTarget {
                identity: opm::process::identity(process),
                id,
            }
        })
        .collect()
}

/// Remove every target by identity, re-reading state between targets.
///
/// Each target is looked up by identity in a fresh snapshot, so an id that moved
/// because another target's removal compacted the list cannot address the wrong
/// process. A target that is already gone is skipped with a warning.
///
/// `reload` must return current state (`Runner::new` — the daemon's cache while
/// it runs), and `remove_by_id` receives the id resolved against that state.
pub(crate) fn remove_targets<F>(
    targets: Vec<RemovalTarget>,
    kind: &str,
    mut reload: impl FnMut() -> Runner,
    mut remove_by_id: F,
) where
    F: FnMut(usize) -> Result<(), String>,
{
    for target in targets {
        let runner = reload();
        let Some(id) = runner.id_of_identity(&target.identity) else {
            println!(
                "{} Skipping {}({}) — no longer exists",
                *helpers::WARN,
                kind,
                target.id
            );
            continue;
        };

        if let Err(err) = remove_by_id(id) {
            crashln!("{} {}", *helpers::FAIL, err);
        }
    }
}

#[cfg(test)]
mod remove_batch_tests {
    use super::*;
    use opm::process::{Crash, Process, Watch};
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    fn process(id: usize, name: &str) -> Process {
        Process {
            id,
            pid: 0,
            shell_pid: None,
            env: BTreeMap::new(),
            name: name.to_string(),
            path: PathBuf::from("/tmp/opm-remove-batch"),
            script: format!("node {name}.js"),
            restarts: 0,
            running: false,
            crash: Crash { crashed: false },
            watch: Watch {
                enabled: false,
                path: String::new(),
                hash: String::new(),
            },
            children: vec![],
            started: chrono::Utc::now(),
            max_memory: 0,
            agent_id: None,
            frozen_until: None,
            last_action_at: chrono::Utc::now(),
            manual_stop: false,
            errored: false,
            last_restart_attempt: None,
            failed_restart_attempts: 0,
            session_id: None,
            process_start_time: None,
            is_process_tree: false,
            log_out: None,
            log_error: None,
        }
    }

    fn runner_with(processes: Vec<Process>) -> Runner {
        let mut runner = Runner::default();
        for item in processes {
            runner.list.insert(item.id, item);
        }
        runner
    }

    fn server() -> String {
        "local".to_string()
    }

    #[test]
    fn remove_batch_resolves_ids_to_identities() {
        let runner = runner_with(vec![
            process(0, "app1"),
            process(1, "app2"),
            process(2, "app3"),
        ]);

        let targets = resolve_removal_targets(&[Item::Id(0), Item::Id(2)], &runner, &server());

        assert_eq!(
            targets,
            vec![
                RemovalTarget {
                    identity: (
                        "app1".to_string(),
                        "node app1.js".to_string(),
                        PathBuf::from("/tmp/opm-remove-batch"),
                    ),
                    id: 0,
                },
                RemovalTarget {
                    identity: (
                        "app3".to_string(),
                        "node app3.js".to_string(),
                        PathBuf::from("/tmp/opm-remove-batch"),
                    ),
                    id: 2,
                },
            ]
        );
    }

    #[test]
    fn remove_batch_resolves_names_to_identities() {
        let runner = runner_with(vec![process(0, "app1"), process(1, "app2")]);

        let targets = resolve_removal_targets(&[Item::Name("app2".to_string())], &runner, &server());

        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].id, 1);
        assert_eq!(targets[0].identity.0, "app2");
    }

    #[test]
    fn remove_batch_addresses_a_target_that_moved_because_another_compacted() {
        let runner = runner_with(vec![
            process(0, "app1"),
            process(1, "app2"),
            process(2, "app3"),
            process(3, "app4"),
        ]);
        let targets = resolve_removal_targets(&[Item::Id(3), Item::Id(2)], &runner, &server());

        let state = std::cell::RefCell::new(runner);
        let mut removed_names = Vec::new();

        remove_targets(
            targets,
            "",
            || state.borrow().clone(),
            |id| {
                let mut live = state.borrow_mut();
                let removed = live.list.remove(&id).expect("resolved id must exist");
                removed_names.push(removed.name);
                live.compact();
                Ok(())
            },
        );

        assert_eq!(
            removed_names,
            vec!["app4".to_string(), "app3".to_string()],
            "each target must be removed by identity after the previous compaction"
        );
        let live = state.borrow();
        let left: Vec<String> = live.list.values().map(|p| p.name.clone()).collect();
        assert_eq!(left, vec!["app1".to_string(), "app2".to_string()]);
    }

    #[test]
    fn remove_batch_skips_a_target_that_no_longer_exists() {
        let runner = runner_with(vec![process(0, "app1"), process(1, "app2")]);
        let targets = resolve_removal_targets(&[Item::Id(0), Item::Id(1)], &runner, &server());

        let state = std::cell::RefCell::new(runner_with(vec![process(0, "app1")]));
        let mut removed_names = Vec::new();

        remove_targets(
            targets,
            "",
            || state.borrow().clone(),
            |id| {
                let mut live = state.borrow_mut();
                let Some(removed) = live.list.remove(&id) else {
                    return Err(format!("Process ({id}) not found"));
                };
                removed_names.push(removed.name);
                live.compact();
                Ok(())
            },
        );

        assert_eq!(
            removed_names,
            vec!["app1".to_string()],
            "the vanished target must be skipped, not delete whatever sits at its old id"
        );
        assert!(state.borrow().list.is_empty());
    }
}
