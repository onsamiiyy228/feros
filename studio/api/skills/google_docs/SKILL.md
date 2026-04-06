---
name: google_docs
display_name: Google Docs
description: Read, create, or edit Google Docs documents
auth_type: oauth2
category: productivity
---

# Google Docs Integration Skill

## When to use
When the user wants their voice agent to create or update Google Docs during or after a call.
Common triggers:
- "create a call summary in Google Docs"
- "append notes to a document after the call"
- "write a report to Google Docs"

## What to do

1. **Check connection** via `check_connection("google_docs")`.

2. **If not connected**: Use `secret("google_docs")` in tool scripts. The system
   will automatically emit the correct action card based on the platform
   configuration. Do NOT emit action cards manually.

3. **Discover documents** (Google Drive supports listing):
   - `api_call("google_docs", "GET", "https://www.googleapis.com/drive/v3/files?q=mimeType='application/vnd.google-apps.document'&fields=files(id,name)")` → list Docs

4. Use the real document ID in the tool config.

## Example tool configs

### Create a new document

```json
{
  "name": "google_docs.create_document",
  "description": "Create a new Google Doc with the call summary",
  "params": [
    {"name": "title", "description": "Document title", "type": "string", "required": true},
    {"name": "content", "description": "Document body text", "type": "string", "required": true}
  ],
  "script": "let key = secret('google_docs');\nlet createResp = http_post_h('https://docs.googleapis.com/v1/documents', {title: title}, {'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'});\nif (createResp.status < 200 || createResp.status >= 300) { throw new Error(`Google Docs ${createResp.status}: ${createResp.body}`); }\nlet docId = JSON.parse(createResp.body).documentId;\nlet requests = [{insertText: {location: {index: 1}, text: content}}];\nlet updateResp = http_post_h('https://docs.googleapis.com/v1/documents/' + docId + ':batchUpdate', {requests: requests}, {'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'});\nif (updateResp.status >= 200 && updateResp.status < 300) { return 'Document created: https://docs.google.com/document/d/' + docId; }\nthrow new Error(`Google Docs update ${updateResp.status}: ${updateResp.body}`);",
  "side_effect": true
}
```

## Rules
- NEVER hardcode OAuth tokens in tool headers
- NEVER ask for credentials in chat
- Always use `secret("google_docs")` for credentials
- Creating a document requires two API calls: create (returns docId) then batchUpdate (insert text)
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body`
