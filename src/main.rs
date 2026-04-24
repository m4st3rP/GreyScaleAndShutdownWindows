mod config;
mod grayscale;
mod ipc;
mod agent;
mod service;
mod config_tool;

use std::env;

fn main() {
    let args: Vec<String> = env::args().collect();
    let mode = args.get(1).map(|s| s.as_str()).unwrap_or("config");

    match mode {
        "service" => {
            if let Err(e) = service::run() {
                eprintln!("Service error: {:?}", e);
            }
        }
        "agent" => {
            agent::run();
        }
        "config" => {
            config_tool::run();
        }
        _ => {
            eprintln!("Unknown mode: {}", mode);
            eprintln!("Valid modes: service, agent, config");
        }
    }
}
