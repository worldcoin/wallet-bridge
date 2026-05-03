-- Atomic insert for the invite-code variant. Returns 1 on success and 0
-- if the index is already occupied (live row), giving us the 409-on-collision
-- guarantee in a single round-trip.
--
-- KEYS[1]: code:idx:<index>
-- ARGV[1]: request_id (UUID string)
-- ARGV[2]: iv (base64)
-- ARGV[3]: payload (base64)
-- ARGV[4]: session_nonce_hash (sha256 hex)
-- ARGV[5]: ttl in seconds

if redis.call("EXISTS", KEYS[1]) == 1 then
    return 0
end
redis.call("HSET", KEYS[1],
    "request_id", ARGV[1],
    "iv", ARGV[2],
    "payload", ARGV[3],
    "session_nonce_hash", ARGV[4],
    "redeemed", "false")
redis.call("EXPIRE", KEYS[1], ARGV[5])
return 1
