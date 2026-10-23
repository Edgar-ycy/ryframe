local expired = {}
for index = 0, 10 do
  local key_offset = index * 3
  local argument_offset = index * 5
  local tenant_key = KEYS[key_offset + 1]
  local user_key = KEYS[key_offset + 2]
  local snapshot_key = KEYS[key_offset + 3]
  if redis.call('TYPE', tenant_key)['ok'] ~= 'string'
      or redis.call('GET', tenant_key) ~= ARGV[argument_offset + 1]
      or redis.call('TYPE', user_key)['ok'] ~= 'string'
      or redis.call('GET', user_key) ~= ARGV[argument_offset + 2] then
    return redis.error_reply('authorization persistent cache compare failed')
  end
  local expected_type = ARGV[argument_offset + 3]
  local snapshot_type = redis.call('TYPE', snapshot_key)['ok']
  if snapshot_type ~= expected_type then
    return redis.error_reply('authorization snapshot cache type changed')
  elseif snapshot_type == 'none' then
    table.insert(expired, snapshot_key)
  elseif snapshot_type == 'hash' then
    if redis.call('HLEN', snapshot_key) ~= 1
        or redis.call('HGET', snapshot_key, ARGV[argument_offset + 4]) ~= ARGV[argument_offset + 5] then
      return redis.error_reply('authorization snapshot cache compare failed')
    end
  else
    return redis.error_reply('authorization snapshot cache type changed')
  end
end
local deleted = redis.call('UNLINK', unpack(KEYS))
return {deleted, expired}