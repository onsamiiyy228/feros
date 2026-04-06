---
name: google_calendar
display_name: Google Calendar
description: Create, read, or check availability on Google Calendar
auth_type: oauth2
category: scheduling
---

# Google Calendar Integration Skill

## When to use
When the user wants their voice agent to interact with Google Calendar
during a phone call. Common triggers:
- "book an appointment on my calendar"
- "check my availability"
- "schedule a meeting for the caller"
- "create a calendar event"
- "look up upcoming appointments"

## What to do

1. **Check connection** via `check_connection("google_calendar")`.

2. **If not connected**: Use `secret("google_calendar")` in tool scripts.
   The system will automatically emit the correct action card (OAuth or
   manual token) based on the platform configuration. Do NOT emit action
   cards manually.

3. **Discover calendars** (Google Calendar supports auto-discovery):
   - `api_call("google_calendar", "GET", "/users/me/calendarList")` → list calendars with IDs

4. Use the real calendar ID. If only one calendar exists, use it.
   If multiple, ask the user which one.

5. **If discovery fails**, default to `"primary"` (the user's main calendar).

## Example tool configs

### Create an event

```json
{
  "name": "google_calendar.create_event",
  "description": "Book an appointment on the calendar",
  "params": [
    {"name": "summary", "description": "Title of the event (e.g. 'Consultation with John')", "type": "string", "required": true},
    {"name": "start_datetime", "description": "Start date and time in ISO 8601 format (e.g. '2025-06-15T10:00:00-05:00')", "type": "string", "required": true},
    {"name": "end_datetime", "description": "End date and time in ISO 8601 format (e.g. '2025-06-15T11:00:00-05:00')", "type": "string", "required": true},
    {"name": "description", "description": "Event description or notes from the call", "type": "string", "required": false},
    {"name": "attendee_email", "description": "Email address of the attendee to invite", "type": "string", "required": false}
  ],
  "script": "let key = secret('google_calendar');\nlet body = {summary: summary, start: {dateTime: start_datetime}, end: {dateTime: end_datetime}};\nif (description) body.description = description;\nif (attendee_email) body.attendees = [{email: attendee_email}];\nlet resp = http_post_h('https://www.googleapis.com/calendar/v3/calendars/primary/events', body, {'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'});\nif (resp.status >= 200 && resp.status < 300) { return 'Appointment booked.'; }\nthrow new Error(`Google Calendar ${resp.status}: ${resp.body}`);",
  "side_effect": true
}
```

### List upcoming events

```json
{
  "name": "google_calendar.list_events",
  "description": "Check upcoming appointments on the calendar",
  "params": [
    {"name": "timeMin", "description": "Start of search window in ISO 8601 format", "type": "string", "required": true},
    {"name": "timeMax", "description": "End of search window in ISO 8601 format", "type": "string", "required": true}
  ],
  "script": "let key = secret('google_calendar');\nlet resp = http_get_h('https://www.googleapis.com/calendar/v3/calendars/primary/events?orderBy=startTime&singleEvents=true&maxResults=5&timeMin=' + encodeURIComponent(timeMin) + '&timeMax=' + encodeURIComponent(timeMax), {'Authorization': 'Bearer ' + key});\nif (resp.status >= 200 && resp.status < 300) { return resp.body; }\nthrow new Error(`Google Calendar ${resp.status}: ${resp.body}`);",
  "side_effect": false
}
```

### Check free/busy

```json
{
  "name": "google_calendar.check_availability",
  "description": "Check if a time slot is available on the calendar",
  "params": [
    {"name": "timeMin", "description": "Start of availability window in ISO 8601 format", "type": "string", "required": true},
    {"name": "timeMax", "description": "End of availability window in ISO 8601 format", "type": "string", "required": true},
    {"name": "calendar_id", "description": "Calendar ID to check (defaults to primary)", "type": "string", "required": false}
  ],
  "script": "let key = secret('google_calendar');\nlet cal = calendar_id || 'primary';\nlet resp = http_post_h('https://www.googleapis.com/calendar/v3/freeBusy', {timeMin: timeMin, timeMax: timeMax, items: [{id: cal}]}, {'Authorization': 'Bearer ' + key, 'Content-Type': 'application/json'});\nif (resp.status >= 200 && resp.status < 300) { return resp.body; }\nthrow new Error(`Google Calendar ${resp.status}: ${resp.body}`);",
  "side_effect": false
}
```

## Rules
- NEVER hardcode OAuth tokens or API keys in tool headers
- NEVER ask for credentials in the chat conversation
- Always use `secret("google_calendar")` to reference credentials
- PREFER using `api_call` to discover the calendar ID
- All datetime values must be in ISO 8601 format with timezone offset
- When creating events, always ask the caller for date, time, and a brief description
- Default event duration to 30 minutes if the caller doesn't specify
- Use `primary` as the default calendar ID unless the user specifies otherwise
- For script tools, treat non-2xx HTTP responses as failures and `throw` using `resp.status`/`resp.body` instead of returning a plain-text error string
