local source, target = KEYS[1], KEYS[2]
local offset = tonumber(ARGV[3])
if offset == 0 and redis.call('EXISTS', target) ~= 0 then
    return redis.error_reply('usage copy temporary key already exists')
end
if offset > 0 and redis.call('ZCARD', target) ~= offset then return -1 end
local rows = redis.call('ZRANGE', source, ARGV[1], ARGV[2], 'WITHSCORES')
if #rows == 0 then return 0 end
if not redis.acl_check_cmd('PEXPIRE', target, 60000)
    or not redis.acl_check_cmd('UNLINK', target) then
    return redis.error_reply('usage copy temporary key permission denied')
end
local args = {}
for i = 1, #rows, 2 do
    args[#args + 1] = rows[i + 1]
    args[#args + 1] = rows[i]
end
-- TTL is established in the same command as the first allocation. A cancelled
-- caller or a process crash cannot leave a permanent scratch key behind.
redis.call('ZADD', target, unpack(args))
redis.call('PEXPIRE', target, 60000)
return #rows / 2
