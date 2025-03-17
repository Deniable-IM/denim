use std::env;

mod account;
mod account_authenticator;
mod availability_listener;
mod chunk_cache;
pub mod database;
mod envelope;
mod error;
pub mod managers;
mod message_cache;
mod persisters;
mod postgres;
mod query;
mod response;
mod server;
#[cfg(test)]
mod test_utils;
mod validators;

#[tokio::main]
pub async fn main() {
    let use_tls = !env::args().any(|arg| arg == "--no-tls");
    println!("Using tls: {}", use_tls);
    server::start_server(use_tls).await.unwrap();
}
