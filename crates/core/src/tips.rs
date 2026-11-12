//! Anti-scam tips of the welcome DM and the safety reminders, shared by the
//! bot and the dashboard's previews.

/// Picture showing where to turn off direct messages from server members.
pub const CLOSE_DMS_PNG: &[u8] = include_bytes!("../assets/images/close-dm.png");

/// `(title, text)` about scams, in French or English.
pub fn scam(french: bool) -> (&'static str, &'static str) {
    if french {
        (
            "⚠️ Attention aux scams",
            "Le staff ne t'enverra jamais de DM en premier et ne te demandera jamais ta seed phrase ou ta clé privée. Ne clique pas sur des liens reçus en DM (faux airdrops, wallet drainers) et vérifie toujours l'URL avant de connecter ton wallet. Si quelqu'un se fait passer pour le staff en privé, c'est un scam, signale-le.",
        )
    } else {
        (
            "⚠️ Beware of scams",
            "Staff will never DM you first and will never ask for your seed phrase or private key. Don't click links sent in DMs (fake airdrops, wallet drainers) and always check the URL before connecting your wallet. If someone impersonates staff in your DMs, it's a scam, report it.",
        )
    }
}

/// `(title, text)` about turning off direct messages.
pub fn dms(french: bool) -> (&'static str, &'static str) {
    if french {
        (
            "🔒 Ferme tes DMs",
            "Paramètres de confidentialité du serveur > désactive \"Autoriser les messages privés des membres du serveur\".",
        )
    } else {
        (
            "🔒 Turn off your DMs",
            "Server Privacy Settings > turn off \"Allow direct messages from server members\".",
        )
    }
}

/// Title of the safety reminder.
pub fn reminder_title(french: bool) -> &'static str {
    if french {
        "Rappel de sécurité"
    } else {
        "Safety reminder"
    }
}
