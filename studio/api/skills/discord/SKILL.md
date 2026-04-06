---
name: discord
display_name: Discord
description: Send messages to Discord channels or DMs
auth_type: oauth2
category: messaging
---

# Discord Integration Skill

## When to use
When the user wants their voice agent to send notifications or alerts to a Discord channel.
Common triggers:
- "notify my Discord server when someone calls"
- "send call summaries to Discord"
- "alert the team on Discord"

## What to do

1. **Check connection** via `check_connection("discord")`.

2. **If not connected**: Use `secret("discord")` in tool scripts. The system
   will automatically emit the correct action card based on the platform
   configuration. Do NOT emit action cards manually.

3. **Get channel ID**: Discord requires a channel ID (not name). Options:
   - Ask the user to right-click the channel → Copy Channel ID (Developer Mode must be enabled)
   - Or use a webhook URL directly (see Webhook example below)

4. **Recommended approach for simple notifications**: Use a Discord Webhook URL
   instead of the bot token — it requires no OAuth and is simpler to set up.

## Example tool config

### Send a channel message (bot token)

```json
{
  "name": "discord.send_message",
  "description": "Send a call notification to a Discord channel",
  "params": [
    {"name": "channel_id", "description": "Discord channel ID", "type": "string", "required": true},
    {"name": "content", "description": "Message text", "type": "string", "required": true}
  ],
  "script": "let key = secret('discord');\nlet resp = http_post_h('https://discord.com/api/v10/channels/' + channel_id + '/messages', {content: content}, {'Authorization': 'Bot ' + key, 'Content-Type': 'application/json'});\nif (resp.status >= 200 && resp.status < 300) { return 'Message sent.'; }\nthrow new Error(`Discord ${resp.status}: ${resp.body}`);",
  "side_effect": true
}
```

## Rules
- NEVER hardcode bot tokens in tool headers
- NEVER ask for credentials in chat
- Always use `secret("discord")` for credentials
- Note: bot token auth uses `Bot ` prefix, NOT `Bearer`
- Channel IDs are 18-digit numbers; help users enable Discord Developer Mode to copy them
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body`
