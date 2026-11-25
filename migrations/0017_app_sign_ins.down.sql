-- Back to migration 16. docs/deploy.md has the whole rollback; run this
-- file before 0016's (and the older ones, to go further). It commits on
-- its own and never refuses.
--
--   psql -v ON_ERROR_STOP=1 -1 -c "SET lock_timeout = '10s'" -f 0017_app_sign_ins.down.sql
--
-- What it loses: in-app sign-ins under way and their codes (16 knows no
-- such purpose; an app that asked for a code asks again after the way up)
-- and the testing cap's counts. Web sign-in and deletion codes stay. Apps
-- already signed in keep their tokens.

DROP TABLE app_sign_in_alerts;
DROP TABLE app_sign_in_unchecked;
DROP TABLE app_sign_ins;

DELETE FROM email_codes WHERE purpose = 'app_sign_in';
ALTER TABLE email_codes DROP CONSTRAINT email_codes_purpose_check;
ALTER TABLE email_codes ADD CONSTRAINT email_codes_purpose_check
    CHECK (purpose IN ('sign_in', 'delete'));

-- See 0011_tin_states.down.sql.
DELETE FROM _sqlx_migrations WHERE version = 17;
