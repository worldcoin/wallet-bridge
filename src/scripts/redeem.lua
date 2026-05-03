-- Atomic one-shot redemption. Returns nil if the row is missing, expired, or
-- already redeemed; otherwise flips `redeemed` to true and returns the stored
-- ciphertext + request_id. The atomicity here is what guarantees the
-- "exactly one winner" property under concurrent redeems — splitting this
-- into separate read + write would reintroduce the race the integration test
-- specifically checks for.
--
-- KEYS[1]: code:idx:<index>
-- Returns: nil, or {request_id, iv, payload}

local f = redis.call("HMGET", KEYS[1], "redeemed", "request_id", "iv", "payload")
if not f[1] or f[1] == "true" then
    return nil
end
redis.call("HSET", KEYS[1], "redeemed", "true")
return {f[2], f[3], f[4]}
