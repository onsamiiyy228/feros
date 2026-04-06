---
name: google_sheets
display_name: Google Sheets
description: Read from or append rows to Google Sheets
auth_type: oauth2
category: database
---

# Google Sheets Integration Skill

## When to use
When the user wants their voice agent to log data to or read from
Google Sheets during a phone call. Common triggers:
- "log calls to a Google Sheet"
- "save form responses to sheets"
- "check availability in my spreadsheet"

## What to do

1. **Check connection** via `check_connection("google_sheets")`.

2. **If not connected**: Use `secret("google_sheets")` in tool scripts.
   The system will automatically emit the correct action card based on the
   platform configuration. Do NOT emit action cards manually.

3. **Discover spreadsheets** (Google supports auto-discovery):
   - `api_call("google_sheets", "GET", "https://www.googleapis.com/drive/v3/files?q=mimeType='application/vnd.google-apps.spreadsheet'&fields=files(id,name)")` → list spreadsheets
   - `api_call("google_sheets", "GET", "https://sheets.googleapis.com/v4/spreadsheets/{id}?fields=sheets.properties")` → list sheet tabs

4. Use the real spreadsheet ID and sheet name in the tool config.

5. **If discovery fails**, ask the user for the spreadsheet URL (they can
   copy-paste it from their browser — the ID is between `/d/` and `/edit`).

## Example tool config

```json
{
  "name": "google_sheets.append_row",
  "description": "Log caller information to the Leads spreadsheet",
  "params": [
    {"name": "Name", "description": "Caller's full name", "type": "string", "required": true},
    {"name": "Phone", "description": "Caller's phone number", "type": "string", "required": true},
    {"name": "Notes", "description": "Call summary", "type": "string", "required": false}
  ],
  "script": "let key = secret('google_sheets');\nlet resp = http_post_h('https://sheets.googleapis.com/v4/spreadsheets/SPREADSHEET_ID/values/Sheet1:append?valueInputOption=USER_ENTERED', {values: [[Name, Phone, Notes]]}, {'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'});\nif (resp.status >= 200 && resp.status < 300) { return 'Recorded.'; }\nthrow new Error(`Google Sheets ${resp.status}: ${resp.body}`);",
  "side_effect": true
}
```

**IMPORTANT**: Replace `SPREADSHEET_ID` and `Sheet1` with real values obtained
from `api_call` discovery.

## Rules
- NEVER hardcode API keys in tool headers
- NEVER ask for credentials in chat
- Always use `secret("google_sheets")` for credentials
- PREFER using `api_call` to discover the spreadsheet ID and sheet name
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body` instead of returning a plain-text error string
