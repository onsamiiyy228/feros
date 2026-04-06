---
name: clickup
display_name: ClickUp
description: Create or update tasks and lists in ClickUp
auth_type: oauth2
category: productivity
---

# ClickUp Integration Skill

## When to use
When the user wants their voice agent to create or manage tasks in ClickUp.
Common triggers:
- "create a task in ClickUp when someone calls"
- "log follow-up tasks after the call"
- "add to my ClickUp project"

## What to do

1. **Check connection** via `check_connection("clickup")`.

2. **If not connected**: Use `secret("clickup")` in tool scripts. The system
   will automatically emit the correct action card based on the platform
   configuration. Do NOT emit action cards manually.

3. **Discover workspace and lists** (ClickUp supports auto-discovery):
   - `api_call("clickup", "GET", "/api/v2/team")` → list workspaces (teams) with IDs
   - `api_call("clickup", "GET", "/api/v2/team/{team_id}/space")` → list spaces
   - `api_call("clickup", "GET", "/api/v2/space/{space_id}/list")` → list task lists

4. Use the real list ID in the tool config.

5. **If discovery fails**, ask the user to copy the list ID from the ClickUp URL.

## Example tool config

```json
{
  "name": "clickup.create_task",
  "description": "Create a follow-up task in ClickUp after a call",
  "params": [
    {"name": "name", "description": "Task name or title", "type": "string", "required": true},
    {"name": "description", "description": "Task description or notes from the call", "type": "string", "required": false},
    {"name": "list_id", "description": "ClickUp list ID to create the task in", "type": "string", "required": true}
  ],
  "script": "let key = secret('clickup');\nlet body = {name: name};\nif (description) body.description = description;\nlet resp = http_post_h('https://api.clickup.com/api/v2/list/' + list_id + '/task', body, {'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'});\nif (resp.status >= 200 && resp.status < 300) { return 'Task created.'; }\nthrow new Error(`ClickUp ${resp.status}: ${resp.body}`);",
  "side_effect": true
}
```

## Rules
- NEVER hardcode OAuth tokens in tool headers
- NEVER ask for credentials in chat
- Always use `secret("clickup")` for credentials
- PREFER using `api_call` to discover the list ID instead of asking the user
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body`
