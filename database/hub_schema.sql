-- Pi hub database. It lives in RAM and is copied to the SD card once a day
-- (hub-node/src/store.rs), so the card isn't worn out by constant writes.
-- Only the last 30 days are kept.
--
-- Every message headed for the PC goes in `outbox`. Rows stay `forwarded = 0`
-- until the PC acks them, so nothing is lost while the PC is off.

CREATE TABLE IF NOT EXISTS outbox (
    id          INTEGER PRIMARY KEY,   -- unix microseconds, always increasing (see store.rs)
    ts          INTEGER NOT NULL,      -- unix seconds
    json        TEXT NOT NULL,         -- uplink::HubMsg
    forwarded   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS outbox_unsent ON outbox (forwarded, id);
CREATE INDEX IF NOT EXISTS outbox_ts ON outbox (ts);

-- The phone app reads history and alerts straight out of the outbox (no
-- second copy of every reading in RAM). These indexes keep that fast.
CREATE INDEX IF NOT EXISTS outbox_readings ON outbox (
    json_extract(json, '$.node_id'), json_extract(json, '$.channel'), ts
) WHERE json_extract(json, '$.type') = 'reading';
CREATE INDEX IF NOT EXISTS outbox_types ON outbox (json_extract(json, '$.type'), id);
