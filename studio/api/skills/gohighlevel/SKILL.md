---
name: gohighlevel
display_name: GoHighLevel
description: Read or update contacts, opportunities, notes, and conversations in GoHighLevel
auth_type: oauth2
category: crm
---

# GoHighLevel Integration Skill

## When to use
When the user wants their voice agent to work with GoHighLevel / HighLevel CRM during or after a call.
Common triggers:
- "create this lead in HighLevel"
- "find the caller in GoHighLevel"
- "add a note after the call"
- "update the opportunity stage"
- "review the existing conversation thread"
- "log a follow-up in the CRM"

## What to do

1. **Check connection** via `check_connection("gohighlevel")`.

2. **If not connected**: Use `secret("gohighlevel")` in tool scripts. The system
   will automatically emit the correct action card based on the platform
   configuration. Do NOT emit action cards manually.
   - The connected secret may come from either OAuth or a Private Integration Token.
   - Never ask the user to paste credentials into chat.

3. **Clarify tenant context** before writing tools:
   - Ask for the `location_id` when the user knows which sub-account should be used.
   - If the user only knows the agency/company, ask for `company_id` and the target
     `location_id`.
   - Do NOT pretend the current runtime can auto-discover `location_id` from OAuth.
   - Most CRM endpoints are effectively location-scoped. Treat `location_id` as the
     required working context for contacts, notes, and opportunities.

4. **Prefer lookup before create**:
   - `POST /contacts/search` to find an existing caller by phone
   - If phone search returns nothing and the user has an email, search again by email
   - Reuse an existing `contact_id` when possible instead of creating duplicates

5. **Use the right API families**:
   - `POST /contacts/search` for caller lookup
   - `POST /contacts/` for lead/contact creation
   - `GET /contacts/{contact_id}/notes` to review past notes
   - `POST /contacts/{contact_id}/notes` for call summaries
   - `GET /conversations/{conversation_id}/messages` to inspect an existing thread
   - `PUT /opportunities/{id}` only when you already know the `opportunity_id`

6. **Be conservative with opportunity updates**:
   - Do NOT invent pipeline IDs, status IDs, or custom field names
   - If the user cannot supply the exact opportunity or stage identifiers, ask first
   - For generic updates, accept a `payload_json` object and pass it through unchanged

7. **Always send** the `Version: 2021-07-28` header on direct HTTP calls.

## Example tool configs

### Search for a contact by phone

```json
{
  "name": "gohighlevel.search_contact_by_phone",
  "description": "Find a GoHighLevel contact by phone number within a location",
  "params": [
    {"name": "location_id", "description": "GoHighLevel location ID", "type": "string", "required": true},
    {"name": "phone", "description": "Phone number to search for", "type": "string", "required": true}
  ],
  "script": "let key = secret('gohighlevel');\nlet body = {locationId: location_id, pageLimit: 1, filters: [{field: 'phone', operator: 'eq', value: phone}]};\nlet resp = http_post_h('https://services.leadconnectorhq.com/contacts/search', body, {'Authorization': 'Bearer ' + key, 'Version': '2021-07-28', 'Content-Type': 'application/json'});\nif (resp.status < 200 || resp.status >= 300) { throw new Error(`GoHighLevel ${resp.status}: ${resp.body}`); }\nlet data = JSON.parse(resp.body);\nlet contacts = data.contacts || data.results || [];\nif (!contacts.length) { return 'No contact found.'; }\nreturn JSON.stringify(contacts[0]);",
  "side_effect": false
}
```

### Create a contact

```json
{
  "name": "gohighlevel.create_contact",
  "description": "Create a new contact in GoHighLevel for the caller",
  "params": [
    {"name": "location_id", "description": "GoHighLevel location ID", "type": "string", "required": true},
    {"name": "first_name", "description": "Contact first name", "type": "string", "required": false},
    {"name": "last_name", "description": "Contact last name", "type": "string", "required": false},
    {"name": "phone", "description": "Contact phone number", "type": "string", "required": true},
    {"name": "email", "description": "Contact email address", "type": "string", "required": false}
  ],
  "script": "let key = secret('gohighlevel');\nlet body = {locationId: location_id, phone: phone};\nif (first_name) body.firstName = first_name;\nif (last_name) body.lastName = last_name;\nif (email) body.email = email;\nlet resp = http_post_h('https://services.leadconnectorhq.com/contacts/', body, {'Authorization': 'Bearer ' + key, 'Version': '2021-07-28', 'Content-Type': 'application/json'});\nif (resp.status >= 200 && resp.status < 300) { return resp.body; }\nthrow new Error(`GoHighLevel ${resp.status}: ${resp.body}`);",
  "side_effect": true
}
```

### List notes for a contact

```json
{
  "name": "gohighlevel.list_contact_notes",
  "description": "Fetch the existing notes attached to a GoHighLevel contact",
  "params": [
    {"name": "contact_id", "description": "GoHighLevel contact ID", "type": "string", "required": true}
  ],
  "script": "let key = secret('gohighlevel');\nlet resp = http_get_h('https://services.leadconnectorhq.com/contacts/' + encodeURIComponent(contact_id) + '/notes', {'Authorization': 'Bearer ' + key, 'Version': '2021-07-28'});\nif (resp.status >= 200 && resp.status < 300) { return resp.body; }\nthrow new Error(`GoHighLevel ${resp.status}: ${resp.body}`);",
  "side_effect": false
}
```

### Add a call note

```json
{
  "name": "gohighlevel.create_note",
  "description": "Attach a call summary note to an existing contact",
  "params": [
    {"name": "contact_id", "description": "GoHighLevel contact ID", "type": "string", "required": true},
    {"name": "note_text", "description": "Call summary or note content", "type": "string", "required": true}
  ],
  "script": "let key = secret('gohighlevel');\nlet resp = http_post_h('https://services.leadconnectorhq.com/contacts/' + encodeURIComponent(contact_id) + '/notes', {body: note_text}, {'Authorization': 'Bearer ' + key, 'Version': '2021-07-28', 'Content-Type': 'application/json'});\nif (resp.status >= 200 && resp.status < 300) { return 'Note created.'; }\nthrow new Error(`GoHighLevel ${resp.status}: ${resp.body}`);",
  "side_effect": true
}
```

### Get messages from a conversation

```json
{
  "name": "gohighlevel.get_conversation_messages",
  "description": "Retrieve messages from an existing GoHighLevel conversation thread",
  "params": [
    {"name": "conversation_id", "description": "GoHighLevel conversation ID", "type": "string", "required": true}
  ],
  "script": "let key = secret('gohighlevel');\nlet resp = http_get_h('https://services.leadconnectorhq.com/conversations/' + encodeURIComponent(conversation_id) + '/messages', {'Authorization': 'Bearer ' + key, 'Version': '2021-07-28'});\nif (resp.status >= 200 && resp.status < 300) { return resp.body; }\nthrow new Error(`GoHighLevel ${resp.status}: ${resp.body}`);",
  "side_effect": false
}
```

### Update an opportunity with a raw payload

```json
{
  "name": "gohighlevel.update_opportunity",
  "description": "Update an existing GoHighLevel opportunity when the exact opportunity ID and payload are known",
  "params": [
    {"name": "opportunity_id", "description": "GoHighLevel opportunity ID", "type": "string", "required": true},
    {"name": "payload_json", "description": "Raw JSON body to send to the opportunity update endpoint", "type": "string", "required": true}
  ],
  "script": "let key = secret('gohighlevel');\nlet payload = JSON.parse(payload_json);\nlet resp = http_put_h('https://services.leadconnectorhq.com/opportunities/' + encodeURIComponent(opportunity_id), payload, {'Authorization': 'Bearer ' + key, 'Version': '2021-07-28', 'Content-Type': 'application/json'});\nif (resp.status >= 200 && resp.status < 300) { return resp.body; }\nthrow new Error(`GoHighLevel ${resp.status}: ${resp.body}`);",
  "side_effect": true
}
```

## Rules
- NEVER hardcode OAuth tokens in tool headers
- NEVER ask for credentials or Private Integration Tokens in chat
- Always use `secret("gohighlevel")` for credentials
- `secret("gohighlevel")` may resolve to either an OAuth access token or a Private Integration Token
- Ask for `location_id` explicitly when the workflow is location-scoped
- Treat `company_id` as optional agency context, not as a substitute for `location_id`
- Reuse an existing `contact_id` instead of creating duplicates when lookup succeeds
- Do NOT invent opportunity stage IDs, pipeline IDs, or custom field names
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body`
