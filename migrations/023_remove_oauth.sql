-- OAuth never had a complete flow or frontend. Remove the dormant plaintext
-- credential table rather than carrying an unaudited security boundary.
DROP TABLE IF EXISTS oauth_accounts;
