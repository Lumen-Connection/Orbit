CREATE VIRTUAL TABLE IF NOT EXISTS history_fts USING fts5(
    source,
    item_id,
    title,
    body
);
