import { t } from './i18n.js';

// Configure only Chrome's packaged extension surfaces. No page script is used.
// One panel per tab (0.5.63): the global panel is switched off and the toolbar
// button no longer opens it; background.js answers the button's click by
// opening a panel for that tab only (panel-tabs.js).
export async function configureAgentPanel(chromeApi) {
  try {
    if (!chromeApi.sidePanel?.setPanelBehavior || !chromeApi.sidePanel?.getPanelBehavior || !chromeApi.sidePanel?.setOptions || !chromeApi.action?.onClicked) {
      throw new Error(t('entry_error_side_panel_unsupported'));
    }
    await chromeApi.sidePanel.setPanelBehavior({ openPanelOnActionClick: false });
    await chromeApi.sidePanel.setOptions({ enabled: false });
    const behavior = await chromeApi.sidePanel.getPanelBehavior();
    if (behavior?.openPanelOnActionClick !== false) throw new Error(t('entry_error_side_panel_unconfirmed'));
    await chromeApi.action.setPopup({ popup: '' });
    return { ready: true, error: null };
  } catch (failure) {
    let error = String(failure?.message || failure);
    try { await chromeApi.action.setPopup({ popup: 'popup.html' }); }
    catch (fallback) { error += ` ${t('entry_error_popup_unavailable', fallback?.message || fallback)}`; }
    return { ready: false, error };
  }
}
