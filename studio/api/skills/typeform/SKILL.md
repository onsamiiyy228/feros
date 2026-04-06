---
name: typeform
display_name: Typeform
description: Retrieve form responses or create forms in Typeform
auth_type: oauth2
category: surveys
---

# Typeform Integration Skill

## When to use
When the user wants their voice agent to retrieve Typeform submissions or look up survey data.
Common triggers:
- "check if this caller already submitted the form"
- "look up survey responses in Typeform"
- "retrieve the latest form submissions"

## What to do

1. **Check connection** via `check_connection("typeform")`.

2. **If not connected**: Use `secret("typeform")` in tool scripts. The system
   will automatically emit the correct action card based on the platform
   configuration. Do NOT emit action cards manually.

3. **Discover forms** (Typeform supports auto-discovery):
   - `api_call("typeform", "GET", "/forms")` → list all forms with IDs and titles

4. Use the real form ID in the tool config.

## Example tool config

```json
{
  "name": "typeform.get_responses",
  "description": "Retrieve the latest responses from a Typeform survey",
  "params": [
    {"name": "form_id", "description": "Typeform form ID", "type": "string", "required": true},
    {"name": "page_size", "description": "Number of responses to return (default 5)", "type": "string", "required": false}
  ],
  "script": "let key = secret('typeform');\nlet n = page_size || '5';\nlet resp = http_get_h('https://api.typeform.com/forms/' + form_id + '/responses?page_size=' + n + '&sort_by=submitted_at&order_by=desc', {'Authorization': 'Bearer ' + key});\nif (resp.status >= 200 && resp.status < 300) { return resp.body; }\nthrow new Error(`Typeform ${resp.status}: ${resp.body}`);",
  "side_effect": false
}
```

## Rules
- NEVER hardcode OAuth tokens in tool headers
- NEVER ask for credentials in chat
- Always use `secret("typeform")` for credentials
- PREFER using `api_call` to discover form IDs instead of asking the user
- Typeform API is read-only for responses; creating submissions programmatically is not supported
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body`
