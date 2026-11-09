CREATE TABLE IF NOT EXISTS brazap_status_views (
    account INTEGER NOT NULL, sender TEXT NOT NULL, id TEXT NOT NULL,
    viewed_at INTEGER NOT NULL, PRIMARY KEY(account,sender,id)
);
