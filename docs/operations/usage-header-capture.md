# Usage header capture

Usage HTTP captures preserve original header values for client requests,
provider requests, provider responses, and client responses. Header maps are
validated as JSON objects but their values are not redacted. Capture settings
and existing access controls still apply.

These records can contain credentials such as Authorization, API keys, and
cookies, as well as session identifiers and client metadata. Restrict access
to usage details, database records, exports, and backups accordingly.

Previously stored `[redacted]` values cannot be recovered. Original values
are available only for new captures after deploying this change.

The request detail drawer reads these values directly from the administrator
usage-detail API, including when bodies are not loaded. No frontend setting
can recover header values that were already replaced during capture.
