DELETE member
FROM user_group_members AS member
JOIN users ON users.id = member.user_id
WHERE LOWER(users.role) = 'admin'
  AND (
    member.group_id = '00000000-0000-0000-0000-000000000001'
    OR member.group_id IN (
      SELECT TRIM(BOTH '"' FROM value)
      FROM system_configs
      WHERE `key` = 'default_user_group_id'
    )
  );
