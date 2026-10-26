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

-- Phones running the mobile app. They pair with the hub (approved here) and
-- talk to it on its phone port; they never connect to this PC.
CREATE TABLE IF NOT EXISTS phones (
    id          INTEGER PRIMARY KEY,       -- phone id in frames on the hub's phone port (u16)
    public_key  TEXT NOT NULL UNIQUE,      -- CRYPTO-REVIEW: Ed25519 key (hex) the hub pins
    name        TEXT NOT NULL,
    paired_at   INTEGER NOT NULL
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
    source      TEXT NOT NULL,             -- 'app' | 'rule:<id>' | 'phone:<id>' | 'recipe:<id>'
    ts          INTEGER NOT NULL,
    -- 'queued' (sent to hub) -> 'sent' (hub transmitted it) -> 'acked' (node
    -- confirmed it applied it), or 'dropped' (hub queue overflowed).
    status      TEXT NOT NULL DEFAULT 'queued'
);

-- Counters that must never go backwards (node ids are never reused, so an
-- old rule or old readings can't attach to a new device).
CREATE TABLE IF NOT EXISTS counters (
    name        TEXT PRIMARY KEY,
    value       INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS alerts (
    id          INTEGER PRIMARY KEY,
    node_id     INTEGER,
    channel     INTEGER,
    code        INTEGER NOT NULL,          -- 1 sensor fault, 2 threshold, 3 rebooted, 100 rule fired, 101 command dropped, 102 pairing refused, 103 hub recipe alert
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

-- The PC used to run its own automation rules. Recipes on the hub replaced
-- them (hub-node/src/recipes.rs), so an old database loses that table.
DROP TABLE IF EXISTS rules;
