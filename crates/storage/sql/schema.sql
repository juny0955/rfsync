CREATE TABLE roots (
    id              INTEGER PRIMARY KEY,
    local_path      TEXT NOT NULL UNIQUE,

    has_snapshot    INTEGER NOT NULL DEFAULT 0
                    CHECK (has_snapshot IN (0, 1) )
) STRICT;

CREATE TABLE folders (
    id              INTEGER PRIMARY KEY,
    root_id         INTEGER NOT NULL
                    REFERENCES roots(id) ON DELETE CASCADE,
    relative_path   TEXT NOT NULL,

    UNIQUE (root_id, relative_path)
) STRICT;

CREATE TABLE files (
    id              INTEGER PRIMARY KEY,
    folder_id       INTEGER NOT NULL
                    REFERENCES folders(id) ON DELETE CASCADE,
    name            TEXT NOT NULL CHECK ( name <> '' ),
    fsize           INTEGER NOT NULL CHECK ( fsize >= 0 ),
    hash            TEXT NOT NULL CHECK ( length(hash) = 64 ),
    modified_secs   INTEGER NOT NULL,
    modified_nanos  INTEGER NOT NULL
                    CHECK ( modified_nanos BETWEEN 0 AND 999999999 ),

    UNIQUE (folder_id, name)
) STRICT;
