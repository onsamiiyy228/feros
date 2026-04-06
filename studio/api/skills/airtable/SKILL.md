---
name: airtable
display_name: Airtable
description: Save or retrieve records from Airtable bases
auth_type: oauth2
category: database
---

# Airtable Integration Skill

## When to use
When the user wants their voice agent to create, read, update, or delete
records in an Airtable base during a phone call. Common triggers:
- "save caller info to Airtable"
- "log leads in a spreadsheet"
- "create a record when someone calls"

## What to do

1. **Check connection** via the connection status in your instructions,
   or call `check_connection("airtable")`.

2. **If not connected**: Use `secret("airtable")` in tool scripts. The system
   will automatically emit the correct action card (OAuth or manual PAT) based
   on the platform configuration. Do NOT emit action cards manually.

3. **Discover schema** (Airtable supports full auto-discovery):
   - `api_call("airtable", "GET", "/meta/bases")` → list all bases
   - `api_call("airtable", "GET", "/meta/bases/{base_id}/tables")` → tables + fields
   - Use the discovered base_id, table name, and field names in tool config.

4. **Analyze schema**: Compare discovered fields with what the agent needs.
   If fields are missing, present numbered options and let the user decide.

5. **If discovery fails** (e.g. 401, timeout), ask the user for the base ID
   and table name directly.

## Example tool config

```json
{
  "name": "airtable.create_record",
  "description": "Save caller information to the Leads table in Airtable",
  "params": [
    {"name": "Name", "description": "Caller's full name", "type": "string", "required": true},
    {"name": "Phone", "description": "Caller's phone number", "type": "string", "required": true},
    {"name": "Notes", "description": "Summary of the call", "type": "string", "required": false}
  ],
  "script": "let key = secret('airtable');\nlet resp = http_post_h('https://api.airtable.com/v0/appXXXXXXXX/Leads', {fields: {Name: Name, Phone: Phone, Notes: Notes}}, {'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'});\nif (resp.status >= 200 && resp.status < 300) { return 'Record saved successfully.'; }\nthrow new Error(`Airtable ${resp.status}: ${resp.body}`);",
  "side_effect": true
}
```

**IMPORTANT**: Replace `appXXXXXXXX` and `Leads` with real values obtained
from `api_call` discovery. The example above is only a template.

## Rules
- NEVER hardcode API keys in tool headers
- NEVER ask for the API key in the chat conversation
- Always use `secret("airtable")` to reference credentials
- PREFER using `api_call` to discover the base_id and table name instead of asking
- Always include `Content-Type: application/json` in headers for POST requests
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body` instead of returning a plain-text error string
- The Airtable API requires parameters nested inside a `"fields"` wrapper. Name
  each tool parameter with the exact Airtable field name (e.g. `Name`, `Phone`)
  so the script can build the `{"fields": {...}}` payload correctly.
