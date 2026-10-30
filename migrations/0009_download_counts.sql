-- How many times each released file was handed out, per day (in Manila).
-- Counted when /download/{platform} or /install-linux.sh sends someone to
-- the file; nothing about who asked is kept. Names are the manifest's
-- (platform macos/windows/linux/android, arch, kind).
CREATE TABLE download_counts (
    day      date   NOT NULL,
    version  text   NOT NULL,
    platform text   NOT NULL,
    arch     text   NOT NULL,
    kind     text   NOT NULL,
    count    bigint NOT NULL CHECK (count > 0),
    PRIMARY KEY (day, version, platform, arch, kind)
);
