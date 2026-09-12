-- Redis scripts do not roll back errors. Complete all predictable checks before XADD.
if #KEYS ~= 2 or #ARGV < 4 or (#ARGV - 2) % 2 ~= 0 then
    return redis.error_reply('ERR invalid pending transfer arguments')
end
if KEYS[1] == KEYS[2] then
    return redis.error_reply('ERR pending transfer source and destination must differ')
end
if type(redis.acl_check_cmd) ~= 'function' then
    return redis.error_reply('ERR pending transfer requires Redis 7 or later for ACL preflight')
end

-- The exact range bounds keep work independent of the size of the PEL.
local pending = redis.call('XPENDING', KEYS[1], ARGV[1], ARGV[2], ARGV[2], 1)
if #pending == 0 then
    return {0, '', 0, 0}
end
if pending[1][1] ~= ARGV[2] then
    return redis.error_reply('ERR pending transfer requires an exact canonical entry ID')
end
local destination_type = redis.call('TYPE', KEYS[2]).ok
if destination_type ~= 'none' and destination_type ~= 'stream' then
    return redis.error_reply('WRONGTYPE pending transfer destination must be a stream')
end

local append = {'XADD', KEYS[2], '*'}
for index = 3, #ARGV do
    append[#append + 1] = ARGV[index]
end
if not redis.acl_check_cmd(unpack(append)) then
    return redis.error_reply('NOPERM pending transfer requires XADD permission')
end
if not redis.acl_check_cmd('XACK', KEYS[1], ARGV[1], ARGV[2]) then
    return redis.error_reply('NOPERM pending transfer requires XACK permission')
end
if not redis.acl_check_cmd('XDEL', KEYS[1], ARGV[2]) then
    return redis.error_reply('NOPERM pending transfer requires XDEL permission')
end

-- PEL membership, stream types and ACLs cannot change between these commands.
-- A trimmed source body is still recoverable from the caller's retained fields.
local destination_id = redis.call(unpack(append))
local acked = redis.call('XACK', KEYS[1], ARGV[1], ARGV[2])
local deleted = redis.call('XDEL', KEYS[1], ARGV[2])
return {1, destination_id, acked, deleted}
