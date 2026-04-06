---
name: lemlist
display_name: Lemlist
description: Create campaigns, add leads, or track email outreach in Lemlist
auth_type: api_key
category: marketing
---

# Lemlist Integration Skill

## When to use
When the user wants their voice agent to add callers as leads to Lemlist outreach campaigns.
Common triggers:
- "add this caller to my Lemlist campaign"
- "enroll the lead in a cold email sequence"
- "add the contact to outreach"

## What to do

1. **Check connection** via `check_connection("lemlist")`.

2. **If not connected**: Use `secret("lemlist")` in tool scripts. The system
   will automatically emit the correct action card based on the platform
   configuration. Do NOT emit action cards manually.

3. **Discover campaigns** (Lemlist supports auto-discovery):
   - `api_call("lemlist", "GET", "/campaigns")` → list campaigns with IDs and names

4. Use the real campaign ID in the tool config.

## Example tool config

```json
{
  "name": "lemlist.add_lead",
  "description": "Add a caller as a lead to a Lemlist campaign",
  "params": [
    {"name": "campaign_id", "description": "Lemlist campaign ID", "type": "string", "required": true},
    {"name": "email", "description": "Lead's email address", "type": "string", "required": true},
    {"name": "first_name", "description": "Lead's first name", "type": "string", "required": false},
    {"name": "last_name", "description": "Lead's last name", "type": "string", "required": false},
    {"name": "company_name", "description": "Lead's company name", "type": "string", "required": false}
  ],
  "script": "let key = secret('lemlist');\nlet body = {email: email};\nif (first_name) body.firstName = first_name;\nif (last_name) body.lastName = last_name;\nif (company_name) body.companyName = company_name;\nlet resp = http_post_h('https://api.lemlist.com/api/campaigns/' + campaign_id + '/leads/' + encodeURIComponent(email), body, {'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'});\nif (resp.status >= 200 && resp.status < 300) { return 'Lead added to campaign.'; }\nthrow new Error(`Lemlist ${resp.status}: ${resp.body}`);",
  "side_effect": true
}
```

## Rules
- NEVER hardcode API keys in tool headers
- NEVER ask for API keys in chat
- Always use `secret("lemlist")` for credentials
- PREFER using `api_call` to discover campaign IDs instead of asking the user
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body`
