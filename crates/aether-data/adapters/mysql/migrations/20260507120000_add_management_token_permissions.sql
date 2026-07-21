ALTER TABLE management_tokens
    ADD COLUMN permissions TEXT NULL AFTER allowed_ips;

