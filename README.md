# denim [![Rust](https://github.com/Deniable-IM/denim/actions/workflows/rust.yml/badge.svg)](https://github.com/Deniable-IM/denim/actions/workflows/rust.yml)

## Setup
### Preliminaries
1. cargo
2. docker-compose
3. sqlx

### Building the server
1. Go into `server`
2. Create a file called `.env` with the following content
```
DATABASE_URL=postgres://root:root@127.0.0.1:5432/signal_db
DATABASE_URL_TEST=postgres://test:test@127.0.0.1:3306/signal_db_test
REDIS_URL=redis://127.0.0.1:6379
SERVER_ADDRESS=127.0.0.1
HTTPS_PORT=443
HTTP_PORT=80
```

if you are on linux and do not want to sudo the program, you can change the HTTPS and HTTP ports to your liking.

3. An additional optional q value parameter can be added to the `.env` file to control the size of the deniable messages, if not present it will default to a value of 0.6
```
Q_VALUE=0.6
```

4. Go into `server/cert`
5. Generate certificates by running the following
```zsh
./generate_cert.sh
```
6. Go back into `server`
7. Start the database by running the following command
```zsh
docker-compose up
```
8. Start the server by running the following command
```zsh
cargo run
```

### Building the client
1. Go into `client`
2. Create a file called `.env` with the following content
```
HTTPS_SERVER_URL=https://localhost:443
HTTP_SERVER_URL=http://localhost:80
CERT_PATH=../server/cert/rootCA.crt
```
3. Start the client by running the following command
```zsh
cargo run <name> <phone number>
```
As an example, two clients should then be created and messages between them will be sent.

### TLS Configuration
If you do not want to use HTTPS and WSS you can run the server and client with `--no-tls` and then they will just communicate over HTTP and WS

## Clean up
### Resetting the server database
1. Go into `server`
2. Close the database and run the following command
```zsh
docker-compose down -v
```

### Resetting the client database
1. Go into `client/client_db` and remove .db files