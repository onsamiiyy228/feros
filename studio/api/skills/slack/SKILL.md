---
name: slack
display_name: Slack
description: Send messages or notifications to Slack channels
auth_type: oauth2
category: messaging
---

# Slack Integration Skill

## When to use
When the user wants their voice agent to send notifications, alerts, or
summaries to a Slack channel during or after a phone call. Common triggers:
- "notify my team on Slack when someone calls"
- "send a summary to Slack after each call"
- "alert the sales channel"

## What to do

1. **Check connection** via `check_connection("slack")`.

2. **If not connected**: Use `secret("slack")` in tool scripts. The system
   will automatically emit the correct action card based on the platform
   configuration. Do NOT emit action cards manually.

3. **Discover channels** (Slack supports auto-discovery):
   - `api_call("slack", "GET", "/conversations.list?types=public_channel,private_channel&limit=100")` → list channels with IDs

4. Use the real channel ID in the tool config.

5. **If discovery fails**, ask the user for the channel name and help them
   find the ID (right-click channel → View channel details → scroll to bottom).

## Example tool config

```json
{
  "name": "slack.post_message",
  "description": "Send a call summary notification to the Slack sales channel",
  "params": [
    {"name": "channel", "description": "Slack channel ID", "type": "string", "required": true},
    {"name": "text", "description": "Message text summarizing the call", "type": "string", "required": true}
  ],
  "script": "let key = secret('slack');\nlet resp = http_post_h('https://slack.com/api/chat.postMessage', {channel: channel, text: text}, {'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'});\nif (resp.status < 200 || resp.status >= 300) { throw new Error(`Slack ${resp.status}: ${resp.body}`); }\nlet data = JSON.parse(resp.body);\nif (!data.ok) { throw new Error(`Slack API error: ${data.error || 'unknown_error'}`); }\nreturn 'Message sent.';",
  "side_effect": true
}
```

## Rules
- NEVER hardcode bot tokens in tool headers
- NEVER ask for tokens in chat
- Always use `secret("slack")` for credentials
- PREFER using `api_call` to discover channel IDs instead of asking
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body`; for APIs like Slack that may return HTTP 200 on business errors, parse the response body and `throw` when `ok` is false
