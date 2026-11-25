-- M3 Actions (docs/05-actions.md): the user an agent row belongs to, so one
-- user's agent never reverts another user's settings.
ALTER TABLE journal ADD COLUMN owner TEXT NOT NULL DEFAULT '';
CREATE INDEX journal_open ON journal(state) WHERE state IN ('pending', 'applied');
CREATE INDEX journal_txn ON journal(txn_id);
