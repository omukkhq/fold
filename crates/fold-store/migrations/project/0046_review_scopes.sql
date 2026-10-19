-- The review lead chooses the scope of the review of a revised version when it assigns the review: fold's suggestion stands unless it chooses one, and a choice always has its reason. The rows stay as they were written, and the newest choice for a version applies.
CREATE TABLE review_scope_choices (
    id INTEGER PRIMARY KEY,
    version_id INTEGER NOT NULL REFERENCES proposal_versions (id),
    scope TEXT NOT NULL CONSTRAINT review_scope_choices_scope_values CHECK (scope IN ('change', 'full')),
    reason TEXT NOT NULL CONSTRAINT review_scope_choices_reason_present CHECK (trim(reason) <> ''),
    chosen_by TEXT NOT NULL,
    chosen_at INTEGER NOT NULL
);

CREATE INDEX review_scope_choices_version ON review_scope_choices (version_id, id);

CREATE TRIGGER review_scope_choices_no_update BEFORE UPDATE ON review_scope_choices
BEGIN
    SELECT RAISE(ABORT, 'review_scope_choices is append-only');
END;

CREATE TRIGGER review_scope_choices_no_delete BEFORE DELETE ON review_scope_choices
BEGIN
    SELECT RAISE(ABORT, 'review_scope_choices is append-only');
END;

-- The scope of a review of a revised version, written with the review: the earlier version with a verdict that the change is read since, fold's suggestion with the rules that made it, and the scope the review followed. A scope other than the suggestion is a choice, which needs its reason; the rule names are only shown, never decided on.
CREATE TABLE review_scopes (
    review_id INTEGER PRIMARY KEY REFERENCES reviews (id),
    since_version INTEGER NOT NULL CONSTRAINT review_scopes_since_positive CHECK (since_version >= 1),
    suggested TEXT NOT NULL CONSTRAINT review_scopes_suggested_values CHECK (suggested IN ('change', 'full')),
    rules TEXT NOT NULL CONSTRAINT review_scopes_rules_list CHECK (json_valid(rules) AND json_type(rules) = 'array'),
    scope TEXT NOT NULL CONSTRAINT review_scopes_scope_values CHECK (scope IN ('change', 'full')),
    reason TEXT CONSTRAINT review_scopes_reason_present CHECK (reason IS NULL OR trim(reason) <> ''),
    CONSTRAINT review_scopes_full_iff_rules CHECK ((suggested = 'full') = (json_array_length(rules) > 0)),
    CONSTRAINT review_scopes_override_has_reason CHECK (scope = suggested OR reason IS NOT NULL)
);

CREATE TRIGGER review_scopes_no_update BEFORE UPDATE ON review_scopes
BEGIN
    SELECT RAISE(ABORT, 'review_scopes is append-only');
END;

CREATE TRIGGER review_scopes_no_delete BEFORE DELETE ON review_scopes
BEGIN
    SELECT RAISE(ABORT, 'review_scopes is append-only');
END;
