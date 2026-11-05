-- Application-owned lifecycle journal. Media frames are transient and never recorded.
CREATE TABLE IF NOT EXISTS brazap_call_sessions (
    account INTEGER NOT NULL,
    id TEXT NOT NULL,
    ended INTEGER NOT NULL DEFAULT 0,
    payload TEXT NOT NULL,
    PRIMARY KEY (account, id)
);
