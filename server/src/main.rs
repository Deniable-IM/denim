use crate::server::signal_server;
use std::env;
mod account;
mod account_authenticator;
mod availability_listener;
mod envelope;
mod error;
pub mod managers;
mod persisters;
mod server;
mod storage;
#[cfg(test)]
mod test_utils;
mod validators;

fn main() {
    // Use a minimum of 4 threads
    let cpus = num_cpus::get().max(4);
    println!("Using {} threads", cpus);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(cpus)
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        let use_tls = !env::args().any(|arg| arg == "--no-tls");
        println!("using tls: {}", use_tls);
        signal_server::start_server(use_tls).await.unwrap();
    })
}
