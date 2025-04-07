CREATE TABLE accounts (
    id                INTEGER PRIMARY KEY NOT NULL GENERATED ALWAYS AS IDENTITY,
    aci               VARCHAR(36) NOT NULL UNIQUE,
    pni               VARCHAR(40) NOT NULL UNIQUE,
    aci_identity_key  BYTEA NOT NULL,
    pni_identity_key  BYTEA NOT NULL,
    phone_number      TEXT NOT NULL UNIQUE
);

CREATE TABLE devices (
    id              INTEGER PRIMARY KEY NOT NULL GENERATED ALWAYS AS IDENTITY,
    owner           INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    device_id       TEXT NOT NULL,
    name            BYTEA NOT NULL,
    auth_token      TEXT NOT NULL,
    salt            TEXT NOT NULL,
    registration_id TEXT NOT NULL,
    pni_registration_id TEXT NOT NULL,
    UNIQUE(device_id, owner)
);

CREATE TABLE device_capabilities (
    id              INTEGER PRIMARY KEY NOT NULL GENERATED ALWAYS AS IDENTITY,
    owner           INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    capability_type INTEGER NOT NULL
);

CREATE TABLE used_device_link_tokens (
    id              INTEGER PRIMARY KEY NOT NULL GENERATED ALWAYS AS IDENTITY,
    device_link_token TEXT NOT NULL UNIQUE
);

CREATE TABLE msq_queue (
    id          INTEGER PRIMARY KEY NOT NULL GENERATED ALWAYS AS IDENTITY,
    receiver    INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    msg         BYTEA NOT NULL
);

CREATE TABLE aci_signed_pre_key_store (
    id          INTEGER PRIMARY KEY NOT NULL GENERATED ALWAYS AS IDENTITY,
    owner       INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    key_id      TEXT NOT NULL,
    public_key  bytea NOT NULL,
    signature   bytea NOT NULL,
    UNIQUE(owner, key_id)
);

CREATE TABLE pni_signed_pre_key_store (
    id          INTEGER PRIMARY KEY NOT NULL GENERATED ALWAYS AS IDENTITY,
    owner       INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    key_id      TEXT NOT NULL,
    public_key  bytea NOT NULL,
    signature   bytea NOT NULL,
    UNIQUE(owner, key_id)
);

CREATE TABLE aci_pq_last_resort_pre_key_store (
    id          INTEGER PRIMARY KEY NOT NULL GENERATED ALWAYS AS IDENTITY,
    owner       INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    key_id      TEXT NOT NULL,
    public_key  bytea NOT NULL,
    signature   bytea NOT NULL,
    UNIQUE(owner, key_id)

);

CREATE TABLE pni_pq_last_resort_pre_key_store (
    id          INTEGER PRIMARY KEY NOT NULL GENERATED ALWAYS AS IDENTITY,
    owner       INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    key_id      TEXT NOT NULL,
    public_key  bytea NOT NULL,
    signature   bytea NOT NULL,
    UNIQUE(owner, key_id)
);

CREATE TABLE device_keys (
    id                          INTEGER PRIMARY KEY NOT NULL GENERATED ALWAYS AS IDENTITY,
    owner                       INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    aci_signed_pre_key          INTEGER REFERENCES aci_signed_pre_key_store(id) ON DELETE CASCADE,
    pni_signed_pre_key          INTEGER REFERENCES pni_signed_pre_key_store(id) ON DELETE CASCADE,
    aci_pq_last_resort_pre_key  INTEGER REFERENCES aci_pq_last_resort_pre_key_store(id) ON DELETE CASCADE,
    pni_pq_last_resort_pre_key  INTEGER REFERENCES pni_pq_last_resort_pre_key_store(id) ON DELETE CASCADE
);

CREATE TABLE one_time_ec_pre_key_store (
    id          INTEGER PRIMARY KEY NOT NULL GENERATED ALWAYS AS IDENTITY,
    owner       INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    key_id      TEXT NOT NULL,
    public_key  bytea NOT NULL,
    UNIQUE(owner, key_id)
);

CREATE TABLE one_time_pq_pre_key_store (
    id          INTEGER PRIMARY KEY NOT NULL GENERATED ALWAYS AS IDENTITY,
    owner       INTEGER NOT NULL REFERENCES devices(id) ON DELETE CASCADE,
    key_id      TEXT NOT NULL,
    public_key  bytea NOT NULL,
    signature   bytea,
    UNIQUE(owner, key_id)
);
    
