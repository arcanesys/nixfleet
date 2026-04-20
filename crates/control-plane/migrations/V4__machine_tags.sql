CREATE TABLE machine_tags (
    machine_id TEXT NOT NULL,
    tag TEXT NOT NULL,
    PRIMARY KEY (machine_id, tag),
    FOREIGN KEY (machine_id) REFERENCES machines(machine_id)
);
CREATE INDEX idx_machine_tags_tag ON machine_tags(tag);
