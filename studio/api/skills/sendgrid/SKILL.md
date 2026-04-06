---
name: sendgrid
display_name: SendGrid
description: Send transactional or marketing emails via SendGrid
auth_type: api_key
category: marketing
---

# SendGrid Integration Skill

## When to use
When the user wants their voice agent to send emails via SendGrid after or during a call.
Common triggers:
- "send a confirmation email to the caller"
- "email a summary after the call"
- "send a follow-up via SendGrid"

## What to do

1. **Check connection** via `check_connection("sendgrid")`.

2. **If not connected**: Use `secret("sendgrid")` in tool scripts. The system
   will automatically emit the correct action card based on the platform
   configuration. Do NOT emit action cards manually.

3. **Ask for sender email**: SendGrid requires a verified sender address.
   Ask the user for their verified `from` email address.

## Example tool config

```json
{
  "name": "sendgrid.send_email",
  "description": "Send a transactional email via SendGrid",
  "params": [
    {"name": "to_email", "description": "Recipient email address", "type": "string", "required": true},
    {"name": "subject", "description": "Email subject line", "type": "string", "required": true},
    {"name": "text_body", "description": "Plain text email body", "type": "string", "required": true},
    {"name": "from_email", "description": "Verified sender email address", "type": "string", "required": true}
  ],
  "script": "let key = secret('sendgrid');\nlet payload = {personalizations: [{to: [{email: to_email}]}], from: {email: from_email}, subject: subject, content: [{type: 'text/plain', value: text_body}]};\nlet resp = http_post_h('https://api.sendgrid.com/v3/mail/send', payload, {'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'});\nif (resp.status === 202 || (resp.status >= 200 && resp.status < 300)) { return 'Email sent.'; }\nthrow new Error(`SendGrid ${resp.status}: ${resp.body}`);",
  "side_effect": true
}
```

## Rules
- NEVER hardcode API keys in tool headers
- NEVER ask for API keys in chat
- Always use `secret("sendgrid")` for credentials
- SendGrid's `/mail/send` returns HTTP 202 Accepted on success
- The `from_email` must be a verified sender in the SendGrid account
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body`
