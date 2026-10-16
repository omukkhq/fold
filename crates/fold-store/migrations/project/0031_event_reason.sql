-- Why the event happened, for the event types whose readers decide on it, so they filter on a column instead of a key of `data`. `data` still holds the reason for agents and the web. Rows from before this column have NULL.
ALTER TABLE events ADD COLUMN reason TEXT;
