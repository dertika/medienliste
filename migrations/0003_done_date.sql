-- Genaues Abschlussdatum (YYYY-MM-DD), optional zusätzlich zum Jahr.
-- Ist es gesetzt, entspricht done_year immer seinem Jahr.
ALTER TABLE user_media ADD COLUMN done_on TEXT
    CHECK (done_on IS NULL OR done_on GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]');
