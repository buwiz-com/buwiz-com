-- Which app each download is of, now that there is more than one. Every
-- count so far is the filing app's.
ALTER TABLE download_counts ADD COLUMN app text NOT NULL DEFAULT 'ebirforms';
ALTER TABLE download_counts DROP CONSTRAINT download_counts_pkey;
ALTER TABLE download_counts ADD PRIMARY KEY (app, day, version, platform, arch, kind);
