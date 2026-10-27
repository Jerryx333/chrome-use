// A generic approval card: a title, a sentence, a few facts and a row of
// choices. The side panel and the options page show per-site approval
// requests with it (#781); any other "a person must decide before the agent
// goes on" prompt can reuse it by passing its own choices.
//
// Extension pages only. Nothing here is ever put into a web page.

/**
 * @param {Document} doc
 * @param {{ title: string, detail?: string, facts?: Array<[string, string]>,
 *           choices: Array<{ id: string, label: string, primary?: boolean }> }} spec
 * @param {(choiceId: string) => Promise<unknown>} onChoose  A rejection shows
 *   its message on the card and re-enables the buttons.
 * @returns {HTMLElement}
 */
export function approvalCard(doc, spec, onChoose) {
  const card = doc.createElement('section');
  card.className = 'approval-card request';
  const title = doc.createElement('h3');
  title.textContent = spec.title;
  card.append(title);
  if (spec.detail) {
    const detail = doc.createElement('p');
    detail.textContent = spec.detail;
    card.append(detail);
  }
  for (const [label, value] of spec.facts || []) {
    if (!value) continue;
    const fact = doc.createElement('p');
    fact.className = 'hint';
    fact.textContent = `${label}: ${value}`;
    card.append(fact);
  }
  const actions = doc.createElement('div');
  actions.className = 'actions';
  const status = doc.createElement('p');
  status.className = 'hint';
  const buttons = [];
  for (const choice of spec.choices) {
    const button = doc.createElement('button');
    button.type = 'button';
    button.textContent = choice.label;
    button.dataset.choice = choice.id;
    if (choice.primary) button.className = 'primary';
    button.addEventListener('click', async () => {
      for (const b of buttons) b.disabled = true;
      status.textContent = '';
      try {
        await onChoose(choice.id);
      } catch (error) {
        status.textContent = String(error?.message || error);
        for (const b of buttons) b.disabled = false;
      }
    });
    buttons.push(button);
    actions.append(button);
  }
  card.append(actions, status);
  return card;
}
