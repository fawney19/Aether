local rule_count = tonumber(table.remove(ARGV, 1))
local swaps = #KEYS - rule_count
local ttls = {}
-- WATCH covers every source, including modifications made by older instances.
-- Check every temporary key and every permission before replacing any source.
for i = 1, swaps do
    local position = rule_count * 3 + 2 + (i - 1) * 2
    local index = tonumber(ARGV[position + 1])
    local live = tonumber(ARGV[position + 2])
    local source, target = KEYS[index], KEYS[rule_count + i]
    local ttl = redis.call('PTTL', source)
    if ttl == 0 or ttl < -1 or redis.call('ZCARD', target) ~= live then return {2} end
    if not redis.acl_check_cmd('UNLINK', source)
        or not redis.acl_check_cmd('RENAME', target, source)
        or not redis.acl_check_cmd('PEXPIRE', target, math.max(1, ttl))
        or not redis.acl_check_cmd('PERSIST', target) then
        return redis.error_reply('usage copy commit permission denied')
    end
    ttls[i] = ttl
end
for i = 1, swaps do
    local position = rule_count * 3 + 2 + (i - 1) * 2
    local index = tonumber(ARGV[position + 1])
    local source, target = KEYS[index], KEYS[rule_count + i]
    if ttls[i] > 0 then
        redis.call('PEXPIRE', target, ttls[i])
    else
        redis.call('PERSIST', target)
    end
    redis.call('UNLINK', source)
    redis.call('RENAME', target, source)
end
for i = #KEYS, rule_count + 1, -1 do KEYS[i] = nil end
ARGV[rule_count * 3 + 3] = 'inline'
-- The original prune/check/consume script follows in this same EXEC/EVAL.
