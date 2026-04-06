---
name: jotform
display_name: JotForm
description: Retrieve form submissions from JotForm
auth_type: api_key
category: surveys
---

# JotForm Integration Skill

## When to use
When the user wants their voice agent to look up JotForm submissions during a call.
Common triggers:
- "check if this caller submitted the form"
- "look up their JotForm response"
- "retrieve the latest form submissions"

## What to do

1. **Check connection** via `check_connection("jotform")`.

2. **If not connected**: Use `secret("jotform")` in tool scripts. The system
   will automatically emit the correct action card based on the platform
   configuration. Do NOT emit action cards manually.

3. **Discover forms** (JotForm supports auto-discovery):
   - `api_call("jotform", "GET", "/user/forms")` → list all forms with IDs and titles

4. Use the real form ID in the tool config.

## Example tool config

```json
{
  "name": "jotform.get_submissions",
  "description": "Retrieve recent submissions from a JotForm form",
  "params": [
    {"name": "form_id", "description": "JotForm form ID", "type": "string", "required": true},
    {"name": "limit", "description": "Number of submissions to return (default 5)", "type": "string", "required": false}
  ],
  "script": "let key = secret('jotform');\nlet n = limit || '5';\nlet resp = http_get_h('https://api.jotform.com/form/' + form_id + '/submissions?limit=' + n + '&orderby=created_at&direction=DESC', {'apikey': key});\nif (resp.status >= 200 && resp.status < 300) { return resp.body; }\nthrow new Error(`JotForm ${resp.status}: ${resp.body}`);",
  "side_effect": false
}
```

## Rules
- NEVER hardcode API keys in tool headers
- NEVER ask for API keys in chat
- Always use `secret("jotform")` for credentials
- JotForm uses `apikey` as the header name (not `Authorization`)
- PREFER using `api_call` to discover form IDs instead of asking the user
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body`
