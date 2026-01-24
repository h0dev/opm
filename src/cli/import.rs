use super::{Item, Items};
use colored::Colorize;
use macros_rs::{crashln, string};
use serde::{Deserialize, Serialize};

use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::prelude::*,
};

use opm::{
    file::Exists,
    helpers,
    process::{Env, Runner},
};

#[derive(Deserialize, Debug)]
struct ProcessWrapper {
    #[serde(alias = "process")]
    list: HashMap<String, Process>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Process {
    script: String,
    server: Option<String>,
    watch: Option<Watch>,
    #[serde(default)]
    env: Env,
    max_memory: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct Watch {
    path: String,
}

impl Process {
    fn get_watch_path(&self) -> Option<String> {
        self.watch.as_ref().and_then(|w| Some(w.path.clone()))
    }
}

pub fn read_hcl(path: &String) {
    let mut servers: Vec<String> = vec![];

    println!("{} Applying action importProcess", *helpers::SUCCESS);

    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(err) => crashln!(
            "{} Cannot read file to import.\n{}",
            *helpers::FAIL,
            string!(err).white()
        ),
    };

    let hcl_parsed: ProcessWrapper = match hcl::from_str(&contents) {
        Ok(hcl) => hcl,
        Err(err) => crashln!(
            "{} Cannot parse imported file.\n{}",
            *helpers::FAIL,
            string!(err).white()
        ),
    };

    for (name, item) in hcl_parsed.list {
        let mut runner = Runner::new();
        let server_name = &item.server.clone().unwrap_or("local".into());
        let (kind, list_name) = super::format(server_name);

        runner = super::Internal {
            id: 0,
            server_name,
            kind: kind.clone(),
            runner: runner.clone(),
        }
        .create(
            &item.script,
            &Some(name.clone()),
            &item.get_watch_path(),
            &item.max_memory,
            true,
        );

        println!("{} Imported {kind}process {name}", *helpers::SUCCESS);

        match runner.find(&name, server_name) {
            Some(id) => {
                let mut p = runner.get(id);
                p.stop();
                p.set_env(item.env);
                p.restart(false);
                // Save via the process item's runner
                p.get_runner().save();
            }
            None => crashln!("{} Failed to write to ({name})", *helpers::FAIL),
        }

        if !servers.contains(&list_name) {
            servers.push(list_name);
        }
    }

    servers
        .iter()
        .for_each(|server| super::Internal::list(&string!("default"), &server));
    println!(
        "{} Applied startProcess to imported items",
        *helpers::SUCCESS
    );
}

pub fn export_hcl(items: &Items, path: &Option<String>) {
    println!("{} Applying action exportProcess", *helpers::SUCCESS);

    let runner = Runner::new();
    let mut process_ids = Vec::new();

    // Handle "all" case
    if items.is_all() {
        // Get all process IDs from the runner
        for id in runner.list.keys() {
            process_ids.push(*id);
        }

        if process_ids.is_empty() {
            crashln!("{} No processes found to export", *helpers::FAIL);
        }
    } else {
        // Collect specific process IDs
        for item in &items.items {
            match item {
                Item::Id(id) => process_ids.push(*id),
                Item::Name(name) => match runner.find(&name, &string!("internal")) {
                    Some(id) => process_ids.push(id),
                    None => crashln!("{} Process ({name}) not found", *helpers::FAIL),
                },
            }
        }
    }

    // Determine output path
    let output_path = if let Some(p) = path {
        p.clone()
    } else if process_ids.len() == 1 {
        let process = runner.try_info(process_ids[0]);
        format!("{}.hcl", process.name)
    } else {
        "processes.hcl".to_string()
    };

    // Check if file already exists and clear it
    if Exists::check(&output_path).file() {
        if let Err(err) = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&output_path)
        {
            crashln!(
                "{} Error clearing existing file.\n{}",
                *helpers::FAIL,
                string!(err).white()
            )
        }
    }

    // Export each process
    let count = process_ids.len();
    for id in &process_ids {
        let process = runner.try_info(*id);
        let mut watch_parsed = None;
        let mut env_parsed = HashMap::new();

        let current_env: HashMap<String, String> = std::env::vars().collect();

        if process.watch.enabled {
            watch_parsed = Some(Watch {
                path: process.watch.path.clone(),
            })
        }

        for (key, value) in process.env.clone() {
            if let Some(current_value) = current_env.get(&key) {
                if current_value != &value {
                    env_parsed.insert(key, value);
                }
            } else {
                env_parsed.insert(key, value);
            }
        }

        // Format max_memory for export (convert bytes to human-readable format)
        let max_memory_str = if process.max_memory > 0 {
            Some(helpers::format_memory(process.max_memory))
        } else {
            None
        };

        let data = hcl::block! {
            process (process.name.clone()) {
                script = (process.script.clone())
                watch = (watch_parsed)
                env = (env_parsed)
                max_memory = (max_memory_str)
            }
        };

        let serialized = hcl::to_string(&data).unwrap();

        // Append to file
        let mut file = match OpenOptions::new()
            .create(true)
            .write(true)
            .append(true)
            .open(&output_path)
        {
            Ok(f) => f,
            Err(err) => crashln!(
                "{} Error opening file.\n{}",
                *helpers::FAIL,
                string!(err).white()
            ),
        };

        if let Err(err) = writeln!(file, "{}", serialized) {
            crashln!(
                "{} Error writing to file.\n{}",
                *helpers::FAIL,
                string!(err).white()
            )
        }
    }

    println!(
        "{} Exported {} process(es) to {}",
        *helpers::SUCCESS,
        count,
        output_path
    );
}
