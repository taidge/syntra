-- Upgrade persisted default branding and native file MIME types.
UPDATE branding_settings
SET app_name = 'Syntra Office'
WHERE app_name IN ('syntra', char(67) || 'ontrix Office');

UPDATE smtp_settings
SET from_name = 'Syntra Office'
WHERE from_name IN ('syntra', char(67) || 'ontrix Office');

WITH legacy_mime(prefix) AS (VALUES ('application/vnd.' || char(99) || 'ontrix.')),
     document_types(kind) AS (VALUES ('writer'), ('calc'), ('slides'), ('whiteboard'))
UPDATE files
SET mime_type = 'application/vnd.syntra.' || substr(mime_type, (SELECT length(prefix) + 1 FROM legacy_mime))
WHERE mime_type IN (SELECT prefix || kind FROM legacy_mime CROSS JOIN document_types);
