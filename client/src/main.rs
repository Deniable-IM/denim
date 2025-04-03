use client::Client;
use dotenv::dotenv;
use regex::Regex;
use server::SignalServer;
use std::{
    collections::HashMap,
    env::{self, var},
    error::Error,
    fs,
    path::{Path, PathBuf},
};
use storage::{database::ClientDB, device::Device};

mod client;
mod contact_manager;
mod encryption;
mod errors;
mod key_manager;
mod persistent_receiver;
mod server;
mod socket_manager;
mod storage;
#[cfg(test)]
mod test_utils;

fn client_db_path() -> String {
    fs::canonicalize(PathBuf::from("./client_db".to_string()))
        .unwrap()
        .into_os_string()
        .into_string()
        .unwrap()
        .replace("\\", "/")
        .trim_start_matches("//?/")
        .to_owned()
}

async fn make_client(
    name: &str,
    phone: &str,
    certificate_path: &Option<String>,
    server_url: &str,
) -> Client<Device, SignalServer> {
    let db_path = client_db_path() + "/" + name + ".db";
    let client = if Path::exists(Path::new(&db_path)) {
        Client::<Device, SignalServer>::login(&db_path, certificate_path, server_url).await
    } else {
        Client::<Device, SignalServer>::register(
            name,
            phone.into(),
            &db_path,
            server_url,
            certificate_path,
        )
        .await
    };
    client.expect("Failed to create client")
}

fn get_server_info() -> (Option<String>, String) {
    let use_tls = !env::args().any(|arg| arg == "--no-tls");
    println!("Using tls: {}", use_tls);
    if use_tls {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Failed to install rustls crypto provider");
        (
            Some(var("CERT_PATH").expect("Could not find CERT_PATH")),
            var("HTTPS_SERVER_URL").expect("Could not find SERVER_URL"),
        )
    } else {
        (
            None,
            var("HTTP_SERVER_URL").expect("Could not find SERVER_URL"),
        )
    }
}

async fn receive_message(
    client: &mut Client<Device, SignalServer>,
    names: &HashMap<String, String>,
    default: &String,
) {
    let msg = client.receive_message().await.expect("Expected Message");
    let name = names
        .get(
            &msg.source_service_id()
                .expect("Failed to decode")
                .service_id_string(),
        )
        .unwrap_or(default);
    let msg_text = msg.try_get_message_as_string().expect("No Text Content");
    println!("{name}: {msg_text}");
}

async fn receive_all_messages(
    client: &mut Client<Device, SignalServer>,
    names: &HashMap<String, String>,
    default: &String,
) {
    while client.has_message().await {
        receive_message(client, names, default).await;
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    dotenv()?;

    let (cert_path, server_url) = get_server_info();
    let mut user = make_client(&args[1], &args[2], &cert_path, &server_url).await;
    println!("Started client with id: {}", &user.aci.service_id_string());

    let mut contact_names: HashMap<String, String> = user
        .storage
        .device
        .lock()
        .await
        .get_all_nicknames()
        .await?
        .iter()
        .map(|name| (name.service_id.service_id_string(), name.name.to_owned()))
        .collect();
    let default_sender = "Unknown Sender".to_owned();

    let send_regex = Regex::new(r"send:(?<alias>\w+):(?<text>(\w+\s)*)").unwrap();
    let denim_regex = Regex::new(r"denim:(?<alias>\w+):(?<text>(\w+\s)*)").unwrap();
    loop {
        println!("Enter command: ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if input.starts_with("send") {
            if let Some(caps) = send_regex.captures(&input) {
                if !user.has_contact(&caps["alias"]).await {
                    match user.get_service_id_from_server(&caps["alias"]).await {
                        Ok(service_id) => {
                            user.add_contact(&caps["alias"], &service_id)
                                .await
                                .expect("No bob?");
                            contact_names
                                .insert(service_id.service_id_string(), caps["alias"].to_owned());
                        }
                        Err(err) => {
                            println!("{}", err);
                            continue;
                        }
                    }
                }

                user.send_message(&caps["text"], &caps["alias"]).await?;
            } else {
                println!("Not valid send command format")
            };
        } else if input.starts_with("denim") {
            if let Some(caps) = denim_regex.captures(&input) {
                if !user.has_contact(&caps["alias"]).await {
                    match user.get_service_id_from_server(&caps["alias"]).await {
                        Ok(service_id) => {
                            //Needs to be converted to retrieve keys deniably here
                            user.add_contact(&caps["alias"], &service_id)
                                .await
                                .expect("No bob?");
                            contact_names
                                .insert(service_id.service_id_string(), caps["alias"].to_owned());
                        }
                        Err(err) => {
                            println!("{}", err);
                            continue;
                        }
                    }
                }

                user.send_deniable_message(&caps["text"], &caps["alias"])
                    .await?;
            } else {
                println!("Not valid deniable send command format")
            };
        } else if input.starts_with("read") {
            receive_all_messages(&mut user, &contact_names, &default_sender).await;
        } else if input.starts_with("help") {
            println!("Supported commands are:");
            println!("  send:{{phone_number}}:{{message}}");
            println!("  denim:{{phone_number}}:{{message}}");
            println!("  read");
            println!("  help");
            println!("  quit");
        } else if input.starts_with("stop") || input.starts_with("quit") {
            break;
        }
        println!("")
    }

    user.disconnect().await;
    Ok(())
}
