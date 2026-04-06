---
name: brevo
display_name: Brevo
description: Send emails, SMS, or manage contacts via Brevo (formerly Sendinblue)
auth_type: api_key
category: marketing
---

# Brevo Integration Skill

## When to use
When the user wants their voice agent to send emails or SMS via Brevo after a call.
Common triggers:
- "send a confirmation email via Brevo"
- "text the caller via Brevo SMS"
- "send a follow-up through Brevo"

## What to do

1. **Check connection** via `check_connection("brevo")`.

2. **If not connected**: Use `secret("brevo")` in tool scripts. The system
   will automatically emit the correct action card based on the platform
   configuration. Do NOT emit action cards manually.

## Example tool configs

### Send an email

```json
{
  "name": "brevo.send_email",
  "description": "Send a transactional email via Brevo",
  "params": [
    {"name": "to_email", "description": "Recipient email address", "type": "string", "required": true},
    {"name": "subject", "description": "Email subject", "type": "string", "required": true},
    {"name": "text", "description": "Plain text email body", "type": "string", "required": true},
    {"name": "from_email", "description": "Sender email address (must be verified in Brevo)", "type": "string", "required": true},
    {"name": "from_name", "description": "Sender display name", "type": "string", "required": false}
  ],
  "script": "let key = secret('brevo');\nlet body = {sender: {email: from_email, name: from_name || ''}, to: [{email: to_email}], subject: subject, textContent: text};\nlet resp = http_post_h('https://api.brevo.com/v3/smtp/email', body, {'api-key': key, 'Content-Type': 'application/json'});\nif (resp.status >= 200 && resp.status < 300) { return 'Email sent.'; }\nthrow new Error(`Brevo ${resp.status}: ${resp.body}`);",
  "side_effect": true
}
```

## Rules
- NEVER hardcode API keys in tool headers
- NEVER ask for API keys in chat
- Always use `secret("brevo")` for credentials
- Brevo uses `api-key` as the header name (not `Authorization`)
- The sender email must be verified in the Brevo account
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body`
