---
name: calendly
display_name: Calendly
description: Check availability, create scheduling links, or list upcoming events in Calendly
auth_type: oauth2
category: scheduling
---

# Calendly Integration Skill

## When to use
When the user wants their voice agent to interact with Calendly scheduling.
Common triggers:
- "send the caller a scheduling link"
- "check my Calendly availability"
- "list upcoming Calendly appointments"
- "book a meeting via Calendly"

## What to do

1. **Check connection** via `check_connection("calendly")`.

2. **If not connected**: Use `secret("calendly")` in tool scripts. The system
   will automatically emit the correct action card based on the platform
   configuration. Do NOT emit action cards manually.

3. **Get current user** (required for most endpoints):
   - `api_call("calendly", "GET", "/users/me")` → returns user URI needed for other calls

4. **Discover event types**:
   - `api_call("calendly", "GET", "/event_types?user={user_uri}")` → list scheduling links/event types

## Example tool configs

### Get scheduling link for an event type

```json
{
  "name": "calendly.get_scheduling_link",
  "description": "Get the Calendly booking URL for a specific event type to share with the caller",
  "params": [
    {"name": "event_type_name", "description": "Name of the event type (e.g. '30 Minute Meeting')", "type": "string", "required": true}
  ],
  "script": "let key = secret('calendly');\nlet userResp = http_get_h('https://api.calendly.com/users/me', {'Authorization': 'Bearer ' + key});\nif (userResp.status < 200 || userResp.status >= 300) { throw new Error(`Calendly ${userResp.status}: ${userResp.body}`); }\nlet userUri = JSON.parse(userResp.body).resource.uri;\nlet etResp = http_get_h('https://api.calendly.com/event_types?user=' + encodeURIComponent(userUri), {'Authorization': 'Bearer ' + key});\nif (etResp.status < 200 || etResp.status >= 300) { throw new Error(`Calendly ${etResp.status}: ${etResp.body}`); }\nlet types = JSON.parse(etResp.body).collection;\nlet match = types.find(t => t.name.toLowerCase().includes(event_type_name.toLowerCase()));\nif (!match) { return 'No matching event type found.'; }\nreturn match.scheduling_url;",
  "side_effect": false
}
```

### List upcoming scheduled events

```json
{
  "name": "calendly.list_events",
  "description": "List upcoming scheduled Calendly events",
  "params": [
    {"name": "count", "description": "Number of events to return (max 20)", "type": "string", "required": false}
  ],
  "script": "let key = secret('calendly');\nlet userResp = http_get_h('https://api.calendly.com/users/me', {'Authorization': 'Bearer ' + key});\nif (userResp.status < 200 || userResp.status >= 300) { throw new Error(`Calendly ${userResp.status}: ${userResp.body}`); }\nlet userUri = JSON.parse(userResp.body).resource.uri;\nlet n = count || '5';\nlet resp = http_get_h('https://api.calendly.com/scheduled_events?user=' + encodeURIComponent(userUri) + '&count=' + n + '&status=active', {'Authorization': 'Bearer ' + key});\nif (resp.status >= 200 && resp.status < 300) { return resp.body; }\nthrow new Error(`Calendly ${resp.status}: ${resp.body}`);",
  "side_effect": false
}
```

## Rules
- NEVER hardcode OAuth tokens in tool headers
- NEVER ask for credentials in chat
- Always use `secret("calendly")` for credentials
- Most endpoints require the user URI — always fetch `/users/me` first
- Calendly does NOT support creating bookings via API; only share scheduling URLs
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body`
