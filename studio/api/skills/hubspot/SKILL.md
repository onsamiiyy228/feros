---
name: hubspot
display_name: HubSpot
description: Read or update CRM contacts, deals, companies, and tickets in HubSpot
auth_type: oauth2
category: crm
---

# HubSpot Integration Skill

## When to use
When the user wants their voice agent to interact with HubSpot CRM during a phone call.
Common triggers:
- "log this lead in HubSpot"
- "look up the contact in our CRM"
- "create a deal when someone calls"
- "update contact notes after the call"

## What to do

1. **Check connection** via `check_connection("hubspot")`.

2. **If not connected**: Use `secret("hubspot")` in tool scripts. The system
   will automatically emit the correct action card based on the platform
   configuration. Do NOT emit action cards manually.

3. **Discover properties** (HubSpot supports auto-discovery):
   - `api_call("hubspot", "GET", "/crm/v3/properties/contacts")` → list contact properties
   - `api_call("hubspot", "GET", "/crm/v3/properties/deals")` → list deal properties

4. Use discovered property names in tool config.

5. **If discovery fails**, use standard HubSpot properties (`firstname`, `lastname`,
   `email`, `phone`, `company`).

## Example tool configs

### Create a contact

```json
{
  "name": "hubspot.create_contact",
  "description": "Create a new CRM contact for the caller",
  "params": [
    {"name": "firstname", "description": "Caller's first name", "type": "string", "required": true},
    {"name": "lastname", "description": "Caller's last name", "type": "string", "required": false},
    {"name": "phone", "description": "Caller's phone number", "type": "string", "required": true},
    {"name": "email", "description": "Caller's email address", "type": "string", "required": false}
  ],
  "script": "let key = secret('hubspot');\nlet props = {firstname: firstname, phone: phone};\nif (lastname) props.lastname = lastname;\nif (email) props.email = email;\nlet resp = http_post_h('https://api.hubapi.com/crm/v3/objects/contacts', {properties: props}, {'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'});\nif (resp.status >= 200 && resp.status < 300) { return 'Contact created.'; }\nthrow new Error(`HubSpot ${resp.status}: ${resp.body}`);",
  "side_effect": true
}
```

### Search for a contact

```json
{
  "name": "hubspot.search_contact",
  "description": "Look up a contact in HubSpot by phone number",
  "params": [
    {"name": "phone", "description": "Phone number to search for", "type": "string", "required": true}
  ],
  "script": "let key = secret('hubspot');\nlet body = {filterGroups: [{filters: [{value: phone, propertyName: 'phone', operator: 'EQ'}]}], properties: ['firstname','lastname','email','phone']};\nlet resp = http_post_h('https://api.hubapi.com/crm/v3/objects/contacts/search', body, {'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'});\nif (resp.status >= 200 && resp.status < 300) { let data = JSON.parse(resp.body); if (data.total === 0) { return 'No contact found.'; } return JSON.stringify(data.results[0].properties); }\nthrow new Error(`HubSpot ${resp.status}: ${resp.body}`);",
  "side_effect": false
}
```

## Rules
- NEVER hardcode OAuth tokens in tool headers
- NEVER ask for credentials in chat
- Always use `secret("hubspot")` for credentials
- PREFER using `api_call` to discover property names before hard-coding them
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body`
- HubSpot properties must be nested inside `{"properties": {...}}` for create/update requests
