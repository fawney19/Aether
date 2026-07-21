DELETE FROM public.user_group_members AS member
USING public.users AS users
WHERE member.user_id = users.id
  AND LOWER(users.role::text) = 'admin'
  AND (
    member.group_id = '00000000-0000-0000-0000-000000000001'
    OR member.group_id IN (
      SELECT value #>> '{}'
      FROM public.system_configs
      WHERE key = 'default_user_group_id'
    )
  );
