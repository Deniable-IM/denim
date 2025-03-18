use crate::server::signal_server;
use std::env;
mod account;
mod account_authenticator;
mod availability_listener;
pub mod database;
mod envelope;
mod error;
pub mod managers;
mod persisters;
mod postgres;
mod server;
#[cfg(test)]
mod test_utils;
mod validators;

#[tokio::main]
pub async fn main() {
    let use_tls = !env::args().any(|arg| arg == "--no-tls");
    println!("Using tls: {}", use_tls);
    signal_server::start_server(use_tls).await.unwrap();
}
