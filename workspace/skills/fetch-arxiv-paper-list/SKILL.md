---
name: fetch-arxiv-paper-list
description: Fetch the newest papers from an arXiv listing page, keep only papers from the most recent date shown, look up each paper with arxiv-paper-lookup, score and rank them, and produce a structured Markdown report.
---

# Fetch arXiv Paper List

Use this skill when the user wants a ranked report of the newest papers from an arXiv listing page such as `https://arxiv.org/list/math.AG/recent`.

## When To Use

- User provides an arXiv list URL and asks for the newest papers
- User wants papers from the latest posting date only
- User wants a ranked digest of recent papers in a category
- User wants structured summaries and scoring across multiple recent arXiv papers

## Inputs

- A target arXiv listing URL, usually in the form `https://arxiv.org/list/<category>/recent`

## Workflow

1. Receive the target URL from the user.
2. Fetch the page with the `http` tool.
3. Parse the listing page and identify the most recent date section on the page.
4. Extract only the papers listed under that most recent date.
5. For each paper:
   - extract the arXiv ID and title
   - invoke the `arxiv-paper-lookup` skill logic to fetch the paper overview first
   - use the retrieved content to obtain abstract, summary, and evaluation
   - score the paper using the scoring framework defined by `arxiv-paper-lookup`
6. Rank all papers by overall score, using confidence and evidence quality as tie-breakers.
7. Produce a structured Markdown report using `references/report-template.md`.
8. If some papers have missing overview coverage, still include them with an explicit availability note.

## Per-Paper Report Requirements

Every paper section in the combined report must explicitly include the same four dimensions:

1. significance
2. methodology
3. conclusions
4. author reputation

Do not omit any of these dimensions in any paper section, even if the available evidence is weak or unavailable. When evidence is missing, state that clearly rather than skipping the dimension.

## Tool Use

- Use the `http` tool to fetch the arXiv listing page.
- Use the `http` tool again for overview or fallback markdown endpoints.
- Reuse the evaluation and scorecard conventions from `arxiv-paper-lookup`.

## Output Requirements

The final answer must be a Markdown report that includes:

- source URL
- latest date found
- number of papers processed
- ranked paper sections in descending score order
- abstract and summary for each paper
- scorecard fields for significance, methodology, conclusions, and author reputation
- strengths, weaknesses, and uncertainty note for each paper

Use the exact section ordering from `references/report-template.md`.

## Error Handling

- If the source URL cannot be fetched, explain the failure and stop.
- If the page format is unusual, extract the best available latest-date grouping and say what assumption was made.
- If overview data is missing for some papers, include those papers with a note and lower confidence.
- Do not fabricate abstracts, scores, or author reputation evidence.

## Validation

Run `skill_validate("fetch-arxiv-paper-list")` after edits.

## Examples

- "Fetch and rank the newest papers from https://arxiv.org/list/math.AG/recent"
- "Give me a markdown report of today's newest cs.LG papers from arXiv, ranked by promise"
- "Score the most recent papers at https://arxiv.org/list/stat.ML/recent"
