-- Remove the abandoned language-integration configuration surface. No
-- runtime consumer or frontend ever used these records, and api_key values
-- were stored in plaintext inside the main database and its backups.
DROP TABLE IF EXISTS integration_settings;
