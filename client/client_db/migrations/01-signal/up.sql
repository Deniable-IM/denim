CREATE TABLE Identity (
  id                  INTEGER PRIMARY KEY,
  aci                 TEXT NOT NULL,
  pni                 TEXT NOT NULL,
  password            TEXT NOT NULL
);

CREATE TABLE IdentityKeys (
  id                  INTEGER PRIMARY KEY,
  public_key          TEXT NOT NULL,
  private_key         TEXT NOT NULL,
  registration_id     UNSIGNED BIG INT NOT NULL
);

CREATE TABLE DeviceIdentityKeyStore (
  id               INTEGER PRIMARY KEY,
  address          TEXT NOT NULL UNIQUE,
  identity_key     TEXT NOT NULL    -- identity key for another device
);

CREATE TABLE DevicePreKeyStore (
  id              INTEGER PRIMARY KEY,
  pre_key_id      UNSIGNED BIG INT NOT NULL UNIQUE,
  pre_key_record  TEXT NOT NULL
);

CREATE TABLE DeviceSignedPreKeyStore (
  id                      INTEGER PRIMARY KEY,
  signed_pre_key_id       UNSIGNED BIG INT NOT NULL UNIQUE,
  signed_pre_key_record   TEXT NOT NULL
);

CREATE TABLE DeviceKyberPreKeyStore (
  id                    INTEGER PRIMARY KEY,
  kyber_pre_key_id      UNSIGNED BIG INT NOT NULL UNIQUE,
  kyber_pre_key_record  TEXT NOT NULL
);

CREATE TABLE DeviceSessionStore (
  id              INTEGER PRIMARY KEY,
  address         TEXT NOT NULL UNIQUE,
  session_record  TEXT NOT NULL
);

CREATE TABLE DeviceSenderKeyStore (
  id                  INTEGER PRIMARY KEY,
  address             TEXT NOT NULL UNIQUE,
  sender_key_record   TEXT NOT NULL
);

CREATE TABLE Contacts (
  id INTEGER PRIMARY KEY,
  service_id TEXT NOT NULL UNIQUE,
  device_ids TEXT NOT NULL
);

CREATE TABLE Nicknames (
  id INTEGER PRIMARY KEY,
  name TEXT NOT NULL,
  service_id TEXT NOT NULL UNIQUE
);
