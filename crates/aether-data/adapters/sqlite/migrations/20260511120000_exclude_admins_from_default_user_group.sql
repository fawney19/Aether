DELETE FROM user_group_members
WHERE user_id IN (
    SELECT id
    FROM users
    WHERE LOWER(role) = 'admin'
)
  AND (
    group_id = '00000000-0000-0000-0000-000000000001'
    OR group_id IN (
      SELECT TRIM(value, '"')
      FROM system_configs
      WHERE key = 'default_user_group_id'
    )
  );
