ALTER TABLE machines ADD COLUMN current_generation TEXT;
ALTER TABLE machines ADD COLUMN last_seen TEXT;
ALTER TABLE machines ADD COLUMN health_status TEXT DEFAULT 'unknown';
