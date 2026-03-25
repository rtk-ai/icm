# ICM — Persistent Memory for AI Agents

This project uses [ICM](https://github.com/rtk-ai/icm) for persistent memory across sessions.

## Recall (before starting work)

Search for relevant past context before each task:

```bash
icm recall "query"                        # search memories
icm recall "query" -t "topic-name"        # filter by topic
```

## Store (MANDATORY triggers)

You MUST call `icm store` when ANY of the following happens:

1. **Error resolved** → `icm store -t errors-resolved -c "description" -i high -k "keyword1,keyword2"`
2. **Architecture/design decision** → `icm store -t decisions-{project} -c "description" -i high`
3. **User preference discovered** → `icm store -t preferences -c "description" -i critical`
4. **Significant task completed** → `icm store -t context-{project} -c "summary" -i high`

Do this BEFORE responding to the user. Not optional.

## Other commands

```bash
icm topics                                # list all topics
icm health                                # topic hygiene audit
icm update <id> -c "updated content"      # edit memory in-place
icm forget <id>                           # delete a memory
```
