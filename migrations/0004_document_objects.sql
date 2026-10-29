-- Claim documents can live in object storage (Cloudflare R2). The row then
-- keeps the object key and nonce; `ciphertext` stays NULL.
ALTER TABLE claim_documents ADD COLUMN object_key text;
