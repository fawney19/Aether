-- Count before reading members so a large window cannot run an unbounded Lua loop.
local count = redis.call('ZCOUNT', KEYS[1], ARGV[1], '+inf')
if count > tonumber(ARGV[2]) then
    return {0, '0', 0}
end

local members = redis.call('ZRANGEBYSCORE', KEYS[1], ARGV[1], '+inf')
local high, low, positive = 0, 0, 0
local max_high, max_low = 18446744073, 709551615
for _, member in ipairs(members) do
    local value = string.match(member, ':([^:]*)$')
    if value and string.match(value, '^%+?%d+$') then
        value = string.gsub(value, '^%+', '')
        value = string.gsub(value, '^0+', '')
        if #value > 0 and (#value < 20 or (#value == 20 and value <= '18446744073709551615')) then
            positive = positive + 1
            -- Two base-1e9 limbs keep every integer operation exactly representable
            -- in Redis Lua's doubles, including values above 2^53 and u64::MAX.
            local split = math.max(0, #value - 9)
            local value_high = tonumber(string.sub(value, 1, split)) or 0
            local value_low = tonumber(string.sub(value, split + 1))
            low = low + value_low
            high = high + value_high + math.floor(low / 1000000000)
            low = low % 1000000000
            if high > max_high or (high == max_high and low > max_low) then
                high, low = max_high, max_low
            end
        end
    end
end

local total = string.format('%.0f', low)
if high > 0 then
    total = string.format('%.0f', high) .. string.format('%09d', low)
end
return {1, total, positive}
