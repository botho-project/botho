/**
 * Spanish message map (issue #1095). Typed against `keyof typeof en`, so a
 * missing/renamed key is a compile error.
 *
 * Wording reuses the web wallet's already-reviewed `es` locale files
 * (`web-wallet/src/locales/es/{wallet,pay,claim,contacts}.json`) for equivalent
 * strings — e.g. "Importe" (amount), "Comisión de red" (network fee),
 * "Destinatario" (recipient), "frase de recuperación" (recovery phrase),
 * "enlace de reclamación" (claim link) — so the Snap does not fork a second,
 * drifting Spanish translation. Strings with no web-wallet equivalent (the
 * receive privacy blurb, the balance/history empty-states, the sweep-fee copy)
 * are translated to match that reviewed register.
 */
import type { en } from './en';

export const es: Record<keyof typeof en, string> = {
  'receive.heading': 'Recibir BTH',
  'receive.body':
    'Comparte esta dirección oculta de Botho para recibir fondos. Se crea en ' +
    'la cadena una salida única y nueva para cada pago, por lo que tu saldo ' +
    'permanece privado.',

  'balance.heading': 'Saldo de Botho',
  'balance.spendable': 'Disponible',

  'common.node': 'Nodo',

  'send.heading': 'Confirmar envío',
  'send.amount': 'Importe',
  'send.networkFee': 'Comisión de red',
  'send.total': 'Total',
  'send.recipient': 'Destinatario',

  'history.heading': 'Historial de transacciones',
  'history.empty': 'Aún no hay transacciones. Los pagos que recibas aparecerán aquí.',
  'history.spent': 'Enviado',
  'history.received': 'Recibido',
  'history.confirmationsOne': '{count} confirmación',
  'history.confirmationsOther': '{count} confirmaciones',
  'history.line': 'Bloque {height} · {confirmations} · {hash}',

  'contacts.heading': 'Contactos',
  'contacts.empty':
    'Aún no hay contactos guardados. Añade uno para reutilizar una dirección ' +
    'de Botho sin volver a pegarla.',

  'claim.previewHeading': 'Enlace de reclamación',
  'claim.confirmHeading': 'Confirmar reclamación',
  'claim.body':
    'Este enlace de reclamación contiene fondos que se transferirán a tu ' +
    'monedero. La comisión de barrido se paga desde el enlace.',
  'claim.empty':
    'No hay nada que reclamar: este enlace está vacío, ya se ha reclamado o ' +
    'aún no se ha confirmado.',
  'claim.claimable': 'Reclamable',
  'claim.sweepFee': 'Comisión de barrido',
  'claim.youReceive': 'Recibes',
  'claim.hint': 'Pista del enlace: {amount} (cosmética: el importe escaneado de arriba es el que manda)',

  'mnemonic.heading': 'Frase de recuperación de Botho',
  'mnemonic.body':
    'Estas 24 palabras se derivan de tu Frase de Recuperación Secreta de ' +
    'MetaMask y otorgan plena autoridad de gasto sobre este monedero de Botho. ' +
    'Anótalas y guárdalas sin conexión. Cualquiera que las vea puede gastar tus fondos.',
  'mnemonic.placeholder': '•••• •••• (se revela después de confirmar) ••••',

  'error.rejectMnemonic': 'El usuario rechazó revelar la frase de recuperación.',
  'error.rejectSend': 'El usuario rechazó el envío.',
  'error.rejectClaim': 'El usuario rechazó la reclamación.',
};
