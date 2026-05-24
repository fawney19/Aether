# S3 Integrated Backup Design

Date: 2026-05-24

This design adds S3-compatible backup support for Aether's existing integrated admin data export. The feature is meant to be operated from the admin system settings page, not by editing process environment variables. It covers configuration data and user data in one recoverable backup artifact, including users, user groups, API keys, wallet snapshots, and the S3 backup configuration itself.

### Current Context

Aether already has three admin data export surfaces. Configuration export is available at `GET /api/admin/system/config/export`, user export is available at `GET /api/admin/system/users/export`, and integrated export is available at `GET /api/admin/system/data/export`. The integrated export is the correct source of truth for this feature because it already combines `config_data` and `user_data` and matches the user's requirement for a configuration and user-data backup.

The existing export behavior is recovery-oriented rather than redacted. Provider keys, OAuth secrets, LDAP passwords, sensitive system configs, and user API keys are exported in a form that can be imported later when the gateway can decrypt the stored values. S3 backup credentials will follow that same product model. A backup file is therefore a sensitive recovery artifact and must be treated like an admin secret.

System settings already use `system_configs` for editable admin configuration. Sensitive keys such as Turnstile secrets are encrypted on write, returned to the frontend as "is set" rather than plaintext, and exported by the admin export path for recoverability. The S3 backup settings should use the same storage and UI pattern.

### Requirements

The admin frontend must allow an operator to configure S3-compatible backup storage from the system settings page. The settings must include whether automatic backup is enabled, endpoint, region, bucket, prefix, access key ID, secret access key, path-style mode, compression format, schedule, and retention count.

The backup schedule must be product-style configuration rather than a raw cron expression. It uses an interval value and an interval unit. The supported units are hours, days, weeks, and months. Hourly schedules anchor on a minute within the hour. Daily schedules anchor on a time of day. Weekly schedules anchor on weekday and time. Monthly schedules anchor on day-of-month and time. Retention is a positive number meaning the newest N Aether backup objects are kept.

The frontend must keep `Secret Access Key` write-only after saving. The page should show whether it is configured but must not display the saved secret. Clearing or replacing it should use the same pattern as other sensitive admin configuration.

The backup content must reuse the integrated export payload. It should not introduce a second data extraction format for users, groups, API keys, or wallet snapshots. S3 backup settings are part of system configuration and will be included in the integrated backup file so a restored instance can continue using the same backup target without manual reconfiguration.

The feature must support manual backup now and scheduled automatic backup. Manual backup validates the saved configuration by running the same backend task once. Scheduled backup uses the saved schedule configuration and runs in the gateway background task runtime.

After a successful upload, the retention policy must delete only Aether-generated backup objects under the configured bucket and prefix. It must not scan or delete arbitrary bucket contents. Backup objects should use a predictable name such as `aether-data-backup-YYYYMMDD-HHMMSS.json.zst`, and cleanup should only consider names that match this pattern.

### Recommended Approach

Use `system_configs` for all backup configuration. This is the smallest change that matches the current system settings architecture, import/export behavior, and sensitive value handling. Add S3 backup keys to the admin config defaults and sensitive key list where appropriate. The secret access key should be encrypted at rest in the database and masked in normal config read responses.

Add a backend backup module with three bounded responsibilities. One part reads and validates the backup configuration from `system_configs`. One part builds the backup payload by calling the existing integrated export builder and serializing it to JSON. One part uploads to S3-compatible storage, records result metadata, lists matching backup objects under the configured prefix, and deletes old matching objects beyond the retention count.

Use the background task system for backup execution. A manual admin endpoint starts a single run, returns the task/run information, and writes audit metadata. A scheduled worker checks the saved schedule and starts a run when due. The run result should include target type, bucket, object key, byte size, sha256, export version, exported-at timestamp, compression format, and retention cleanup count.

Add the frontend controls near the existing Data Management section. The operator should be able to save the backup configuration, see whether credentials are configured, trigger an immediate backup, and see a clear success or failure message. Existing task history can be used for detailed execution status if it already exposes the needed task events; otherwise the first version can return the created task/run payload and rely on toast feedback plus backend task records.

### Alternatives Considered

Environment-only configuration was rejected because the user wants backup settings editable from the admin UI. It would also make restore less convenient because the backup target would not travel with the exported configuration.

A new `backup_configs` table would isolate backup settings from generic system configuration, but it would add migrations, repository methods, frontend API surfaces, and import/export handling that duplicate an existing pattern. The current feature does not need that extra persistence boundary.

Raw cron expressions would be flexible but are not a good fit for this admin page. The interval unit model with hours, days, weeks, and months is easier to validate, easier to localize in the UI, and covers the intended backup schedules.

Hand-written local filesystem backup files are not part of the design. The task should generate, compress, and upload in memory or through bounded streaming. Any temporary file introduced later for memory control must be automatically cleaned up by the task and must not leave backup artifacts on disk.

### Data Flow

When an operator saves backup settings, the frontend writes normal configuration values through the existing system config API. Non-sensitive values are stored directly. The secret access key is sent only when being set or replaced, and the backend encrypts it using the existing data encryption key.

When a manual or scheduled backup starts, the backend reads the saved settings, validates required fields, builds the integrated export payload, serializes it, compresses it, computes sha256, and uploads it to the configured S3-compatible target. After upload succeeds, it lists objects under the configured prefix, filters to Aether backup object names, sorts them newest first, and deletes objects older than the configured retention count.

When a restored instance imports a backup, the S3 backup configuration is imported as part of `system_configs`, consistent with current recoverable export behavior. The restored instance can continue using the backup target if its data encryption setup can handle the imported sensitive values in the same way as other sensitive Aether configuration.

### Error Handling

Configuration validation should fail before starting upload when required S3 fields are missing, the retention count is invalid, or the schedule is invalid. Manual backup should return a clear admin-facing error if automatic backup is disabled only when the design explicitly requires enabled state; otherwise manual backup may run as long as storage settings are complete.

Upload failures must leave the old backup set untouched. Retention deletion should only run after a successful upload. If upload succeeds but old-object cleanup partially fails, the backup task should still report the uploaded object and include the cleanup error in result metadata or task events so the operator can see that the backup exists but retention enforcement needs attention.

Schedule evaluation should avoid duplicate runs for the same scheduled slot. The implementation should persist enough last-run or slot information in system config or task metadata to avoid starting repeated backups during the same due window after worker ticks or process restarts.

### Testing Plan

Backend tests should cover schedule validation and due-time calculation for hourly, daily, weekly, and monthly schedules. They should cover config parsing, including masking and sensitive handling for the secret access key. Route classification and handler tests should cover the manual backup endpoint. Backup executor tests should use a fake object-store client to verify object key naming, result metadata, and retention filtering so no real network is required.

Export tests should confirm that S3 backup config participates in the integrated export consistently with other system config entries. Frontend tests should cover form state, save payloads, secret write-only behavior, interval-unit conditional fields, and the manual backup action.

### Dependency Decision

Use the `object_store` crate with S3 support for the first implementation. The workspace does not currently include an S3 client, and `object_store` gives Aether a maintained S3-compatible abstraction without hand-writing Signature V4. The implementation should wrap it behind a small local trait so executor and retention tests can use a fake object-store client without real network access.

### Confidence Notes

The highest-confidence decision is reusing the existing integrated export as the backup payload because it already covers the requested data. The second high-confidence decision is storing backup settings in `system_configs` because that matches the current admin settings and import/export architecture. The S3 client choice has moderate dependency risk because it adds a new workspace dependency, but using `object_store` is still safer than building and maintaining a custom S3 signing implementation.
