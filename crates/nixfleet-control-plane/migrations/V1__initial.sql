-- Placeholder migration. Real schema lands in PR 2.
-- Every column in this file carries a "-- derivable from:" comment
-- per docs/CONTRACTS.md §IV (storage purity rule).
CREATE TABLE schema_placeholder (
    id INTEGER PRIMARY KEY    -- derivable from: row ordinal, local only
);
