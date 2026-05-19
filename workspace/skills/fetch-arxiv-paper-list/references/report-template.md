# Paper List Report Template

Use this structure for every report.

## Source
- Target URL: {TARGET_URL}
- Most recent publication date found: {LATEST_DATE}
- Papers discovered on that date: {PAPER_COUNT}

## Ranked Papers

Repeat the following block for each paper in descending rank order.

### {RANK}. {TITLE}
- arXiv ID: {ARXIV_ID}
- arXiv URL: {ARXIV_URL}
- AlphaXiv overview: {ALPHAXIV_OVERVIEW_URL}
- Overall score: {OVERALL_SCORE}
- Verdict: {VERDICT}
- Confidence: {CONFIDENCE}

#### Abstract
{ABSTRACT}

#### Summary
{SUMMARY}

#### Mandatory Evaluation Dimensions

All four dimensions below must be present in every per-paper section. Do not omit any dimension, even when evidence is weak. If evidence is limited, keep the dimension and mark the notes as low-confidence or provisional.

#### Scorecard
| Dimension | Score | Notes |
| --- | ---: | --- |
| Problem significance | {PROBLEM_SIGNIFICANCE} | {PROBLEM_NOTES} |
| Methodological innovativeness | {METHODOLOGY_SCORE} | {METHODOLOGY_NOTES} |
| Conclusion value | {CONCLUSION_SCORE} | {CONCLUSION_NOTES} |
| Author reputation | {AUTHOR_SCORE} | {AUTHOR_NOTES} |

#### Dimension Completeness Check
- Problem significance included: {YES_NO}
- Methodological innovativeness included: {YES_NO}
- Conclusion value included: {YES_NO}
- Author reputation included: {YES_NO}

#### Strengths
- {STRENGTH_1}
- {STRENGTH_2}

#### Weaknesses / Risks
- {WEAKNESS_1}
- {WEAKNESS_2}

#### Uncertainty Note
{UNCERTAINTY_NOTE}

## Method Notes
- Only include papers from the most recent date shown on the source page.
- Rankings should be based on the scoring returned by the `arxiv-paper-lookup` skill.
- Every per-paper section must explicitly include all four mandatory evaluation dimensions: problem significance, methodological innovativeness, conclusion value, and author reputation.
- If evidence is weak for a dimension, especially author reputation, keep it in the report and mark it as provisional rather than omitting it.
- If AlphaXiv data is unavailable for a paper, note that explicitly and rank it lower unless evidence supports otherwise.
