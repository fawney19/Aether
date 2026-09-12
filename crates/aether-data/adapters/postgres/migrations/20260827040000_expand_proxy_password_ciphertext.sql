-- Purpose-bound Fernet envelopes can exceed the former 500-character plaintext limit.
ALTER TABLE proxy_nodes
    ALTER COLUMN proxy_password TYPE TEXT;
