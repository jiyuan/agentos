---
name: arxiv-paper-lookup
description: Look up arXiv papers on alphaxiv.org to fetch structured AI-generated overviews or full extracted markdown, then summarize, critique, rank, and evaluate the paper’s significance, methodology, conclusions, and author reputation. Use when the user asks for a paper review, scorecard, ranking, critique, or a consistent evaluation format for an arXiv or AlphaXiv paper.
---

# ArXiv Paper Lookup

Look up any arXiv paper on alphaxiv.org to get a structured AI-generated overview. Use this when a user shares an arXiv link, an AlphaXiv overview link, or just a paper ID and wants a summary, explanation, critique, ranking, or evaluation.

## When To Use

- User shares an arXiv URL such as `https://arxiv.org/abs/2401.12345`
- User shares a PDF URL such as `https://arxiv.org/pdf/2401.12345`
- User provides a paper ID such as `2401.12345` or `2401.12345v2`
- User shares an AlphaXiv URL such as `https://alphaxiv.org/overview/2401.12345`
- User asks to summarize, explain, critique, analyze, rank, or review a research paper
- User asks whether a paper is important, innovative, credible, worth reading, or how it scores
- User wants a consistent paper review scorecard format across multiple papers

## Inputs

Accept any of the following:

- arXiv abstract URL
- arXiv PDF URL
- AlphaXiv overview URL
- Raw arXiv paper ID

## Workflow

1. Extract the paper ID from the user input.
2. Fetch the machine-readable overview at `https://alphaxiv.org/overview/{PAPER_ID}.md`.
3. Use the overview as the primary source for answering the user.
4. If the overview does not contain enough detail for the user’s question, fetch the full extracted paper text at `https://alphaxiv.org/abs/{PAPER_ID}.md`.
5. If the user asks for evaluation, ranking, or a review, use `references/evaluation-rubric.md` and `references/scorecard-template.md` to produce the scorecard.
6. Keep the review in the template’s order and labels so outputs are comparable across papers.
7. Provide an overall ranking with a confidence level and a short verdict.
8. If evidence for author reputation is weak, explicitly mark that part as provisional.
9. If AlphaXiv content is unavailable, tell the user and fall back to the arXiv PDF URL.

## Mandatory Evaluation Dimensions

Whenever the user asks for a review, ranking, critique, or scored assessment, the output must explicitly evaluate all four of these dimensions:

1. Problem significance
2. Methodological innovativeness
3. Conclusion value
4. Author reputation

These four dimensions are mandatory for review-style outputs, even if the user mentions only one or two of them. If evidence is weak for any dimension, especially author reputation, keep the dimension in the scorecard and mark it as low-confidence or provisional rather than omitting it.

## Tool Use

- Use the `http` tool to GET the AlphaXiv markdown endpoints.
- Prefer the overview endpoint first because it is shorter and optimized for summarization.
- Only fetch the full paper markdown when needed.
- If the user explicitly wants author reputation evaluated more rigorously, you may fetch directly relevant public pages only when necessary and clearly label external evidence.

## Evaluation Rules

- Base judgments primarily on the paper content.
- Distinguish clearly between summary and evaluation.
- Do not invent citation counts, awards, institutional prestige, or author track records.
- Treat author reputation as low-confidence unless directly supported by the paper or fetched evidence.
- Be cautious about overrating novelty when the method appears to be a recombination of known techniques.
- When producing a review, follow the reusable scorecard template in `references/scorecard-template.md`.
- For any review, ranking, critique, or scored assessment, always include all four mandatory evaluation dimensions.

See `references/evaluation-rubric.md` for the scoring framework.

## Error Handling

- If `overview/{PAPER_ID}.md` returns 404, explain that the overview is not available yet.
- If `abs/{PAPER_ID}.md` returns 404, explain that the extracted full text is not available yet.
- If both are unavailable, provide the arXiv PDF link as a fallback.

## Examples

- "Summarize arXiv:2401.12345"
- "Explain this paper: https://arxiv.org/abs/2401.12345"
- "What does https://alphaxiv.org/overview/2401.12345 say about the method?"
- "Read 2401.12345v2 and tell me the key contributions"
- "Rank this paper by significance and novelty: 2401.12345"
- "Evaluate whether this paper is worth reading and assess the authors' reputation"
- "Give me a consistent scorecard review for this paper"

## Validation

Run `skill_validate("arxiv-paper-lookup")` after edits.
