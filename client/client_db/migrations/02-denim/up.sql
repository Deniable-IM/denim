CREATE TABLE DeniableDeviceSessionStore (
  id              INTEGER PRIMARY KEY,
  address         TEXT NOT NULL UNIQUE,
  session_record  TEXT NOT NULL
);

CREATE TABLE DeniableMessage (
  id              INTEGER PRIMARY KEY,
  content         BLOB NOT NULL
);