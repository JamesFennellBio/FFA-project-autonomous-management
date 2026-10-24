-- PC daemon database (full history). Loaded by pc-daemon/src/db.rs at startup.
-- Values are integers in "milli" units: milli-degrees C for temperature,
-- millivolts for analog inputs, 0/1 for digital pins.

PRAGMA foreign_keys = ON;

-- The Pi hub paired to this PC.
CREATE TABLE IF NOT EXISTS hubs (
    id          INTEGER PRIMARY KEY,
    public_key  TEXT NOT NULL UNIQUE,      -- CRYPTO-REVIEW: pinned Ed25519 key (hex)
    name        TEXT NOT NULL DEFAULT 'Hub',
    paired_at   INTEGER NOT NULL,
    last_seen   INTEGER,
    last_seq    INTEGER NOT NULL DEFAULT 0 -- highest hub row id stored (dedupe after reconnect)
);

-- ESP32 data nodes.
CREATE TABLE IF NOT EXISTS nodes (
    id          INTEGER PRIMARY KEY,       -- node_id used in radio frames (u16)
    public_key  TEXT NOT NULL UNIQUE,      -- CRYPTO-REVIEW: pinned Ed25519 key (hex)
    name        TEXT NOT NULL,
    paired_at   INTEGER NOT NULL,
    last_seen   INTEGER,
    transport   TEXT,                      -- 'wifi' | 'lora'
    sessions    INTEGER NOT NULL DEFAULT 0 -- number of ECDH handshakes seen
);

-- One row per pin the node reported in its Announce.
CREATE TABLE IF NOT EXISTS channels (
    node_id     INTEGER NOT NULL REFERENCES nodes(id) ON DELETE CASCADE,
    channel     INTEGER NOT NULL,
    kind        INTEGER NOT NULL,          -- 0 DigitalIn, 1 DigitalOut, 2 AnalogIn, 3 OneWireTemp
    name        TEXT NOT NULL,
    last_value  INTEGER,
    last_ts     INTEGER,
    PRIMARY KEY (node_id, channel)
);

CREATE TABLE IF NOT EXISTS readings (
    id          INTEGER PRIMARY KEY,
    node_id     INTEGER NOT NULL,
    channel     INTEGER NOT NULL,
    value       INTEGER NOT NULL,
    ts          INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS readings_by_channel ON readings (node_id, channel, ts);

CREATE TABLE IF NOT EXISTS commands (
    id          INTEGER PRIMARY KEY,
    node_id     INTEGER NOT NULL,
    channel     INTEGER NOT NULL,
    value       INTEGER NOT NULL,
    source      TEXT NOT NULL,             -- 'app' | 'rule:<id>'
    ts          INTEGER NOT NULL,
    delivered   INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS alerts (
    id          INTEGER PRIMARY KEY,
    node_id     INTEGER,
    channel     INTEGER,
    code        INTEGER NOT NULL,          -- 1 sensor fault, 2 threshold, 3 rebooted, 100 rule fired
    value       INTEGER,
    message     TEXT NOT NULL,
    ts          INTEGER NOT NULL,
    acked       INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS messages (
    id          INTEGER PRIMARY KEY,
    node_id     INTEGER NOT NULL,
    text        TEXT NOT NULL,
    ts          INTEGER NOT NULL
);

-- Automation: "if <node.channel> <op> <threshold> then set <target> to on_value
-- (and back to off_value when it clears)".
CREATE TABLE IF NOT EXISTS rules (
    id              INTEGER PRIMARY KEY,
    name            TEXT NOT NULL,
    enabled         INTEGER NOT NULL DEFAULT 1,
    node_id         INTEGER NOT NULL,
    channel         INTEGER NOT NULL,
    op              TEXT NOT NULL CHECK (op IN ('gt', 'lt')),
    threshold       INTEGER NOT NULL,
    target_node     INTEGER NOT NULL,
    target_channel  INTEGER NOT NULL,
    on_value        INTEGER NOT NULL DEFAULT 1,
    off_value       INTEGER,               -- NULL = leave it on until a person turns it off
    active          INTEGER NOT NULL DEFAULT 0
);
