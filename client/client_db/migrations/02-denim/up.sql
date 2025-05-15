CREATE TABLE DeniableDeviceIdentityKeyStore (
  id               INTEGER PRIMARY KEY,
  address          TEXT NOT NULL UNIQUE,
  identity_key     TEXT NOT NULL
);

CREATE TABLE DeniableDeviceSessionStore (
  id              INTEGER PRIMARY KEY,
  address         TEXT NOT NULL UNIQUE,
  session_record  TEXT NOT NULL
);

CREATE TABLE DeniablePayload (
  id              INTEGER PRIMARY KEY,
  content         BLOB NOT NULL,
  chunk_count     INTEGER NOT NULL
);

CREATE TABLE DeniableKeyRequestsSent (
  id              INTEGER PRIMARY KEY,
  service_id      TEXT NOT NULL UNIQUE,
  alias           TEXT NOT NULL
);

CREATE TABLE DeniableMessageAwaitingEncryption (
  id              INTEGER PRIMARY KEY,
  message         TEXT NOT NULL,
  alias           TEXT NOT NULL
);

CREATE TABLE IncomingDeniableChunk (
  id              INTEGER PRIMARY KEY,
  chunk           BLOB NOT NULL,
  flags           INTEGER NOT NULL
);
