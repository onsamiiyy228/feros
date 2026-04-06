---
name: pagerduty
display_name: PagerDuty
description: Trigger, acknowledge, or resolve PagerDuty incidents
auth_type: oauth2
category: dev-tools
---

# PagerDuty Integration Skill

## When to use
When the user wants their voice agent to trigger or manage PagerDuty incidents.
Common triggers:
- "page the on-call engineer when there's an outage reported"
- "trigger a PagerDuty incident during the call"
- "acknowledge the alert"
- "resolve the incident"

## What to do

1. **Check connection** via `check_connection("pagerduty")`.

2. **If not connected**: Use `secret("pagerduty")` in tool scripts. The system
   will automatically emit the correct action card based on the platform
   configuration. Do NOT emit action cards manually.

3. **Discover services** (PagerDuty supports auto-discovery):
   - `api_call("pagerduty", "GET", "/services?limit=25")` → list services with IDs

4. Use the real service ID in the tool config.

## Example tool config

```json
{
  "name": "pagerduty.trigger_incident",
  "description": "Trigger a PagerDuty incident to alert the on-call engineer",
  "params": [
    {"name": "title", "description": "Incident title / summary of the issue", "type": "string", "required": true},
    {"name": "service_id", "description": "PagerDuty service ID to route the incident to", "type": "string", "required": true},
    {"name": "details", "description": "Additional details about the incident", "type": "string", "required": false}
  ],
  "script": "let key = secret('pagerduty');\nlet incident = {incident: {type: 'incident', title: title, service: {id: service_id, type: 'service_reference'}}};\nif (details) incident.incident.body = {type: 'incident_body', details: details};\nlet resp = http_post_h('https://api.pagerduty.com/incidents', incident, {'Authorization': 'Token token=' + key, 'Content-Type': 'application/json', 'From': 'voice-agent@example.com', 'Accept': 'application/vnd.pagerduty+json;version=2'});\nif (resp.status >= 200 && resp.status < 300) { return 'Incident triggered.'; }\nthrow new Error(`PagerDuty ${resp.status}: ${resp.body}`);",
  "side_effect": true
}
```

## Rules
- NEVER hardcode tokens in tool headers
- NEVER ask for credentials in chat
- Always use `secret("pagerduty")` for credentials
- PagerDuty auth header format is `Token token=<key>` (not `Bearer`)
- The `From` header is required for incident creation — ask the user for the email to use, or use a placeholder
- The `Accept` header `application/vnd.pagerduty+json;version=2` is required
- PREFER using `api_call` to discover service IDs instead of asking the user
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body`
