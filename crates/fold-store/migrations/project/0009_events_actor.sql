-- The newest events of one agent's workers are read by actor, newest first.
CREATE INDEX events_actor_seq ON events (actor, seq);
