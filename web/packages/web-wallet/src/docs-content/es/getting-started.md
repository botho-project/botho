## Primeros pasos con Botho

Botho es una criptomoneda centrada en la privacidad, diseñada para la era poscuántica. Combina **direcciones sigilosas** para la privacidad de las transacciones con el **Protocolo de Consenso Stellar (SCP)** para lograr un consenso rápido y eficiente en energía. A diferencia de las criptomonedas de prueba de trabajo, Botho alcanza la firmeza en segundos manteniendo sólidas garantías de privacidad.

### ¿Qué hace diferente a Botho?

Las criptomonedas tradicionales como Bitcoin tienen cadenas de bloques transparentes en las que cualquiera puede rastrear el flujo de fondos entre direcciones. Incluso las llamadas "monedas de privacidad" a menudo dependen de supuestos criptográficos que los futuros ordenadores cuánticos podrían romper.

Botho adopta un enfoque distinto:

- Las **direcciones sigilosas** garantizan que cada pago que recibes vaya a una dirección única de un solo uso, lo que hace imposible vincular tus transacciones entre sí observando la cadena de bloques
- La **criptografía poscuántica** protege tu privacidad frente a adversarios con ordenadores cuánticos
- El **Acuerdo Bizantino Federado** proporciona firmeza rápida: la seguridad del consenso nunca depende del poder de cómputo (hashpower)
- La **emisión igualitaria** distribuye monedas nuevas mediante minería de CPU con RandomX, deliberadamente desacoplada del consenso: minar genera recompensas pero no otorga voz sobre qué transacciones se confirman
- La **economía progresiva**: las comisiones escalan de 1× a 6× según la concentración de riqueza, el 80 % de cada comisión se redistribuye por lotería y el 20 % se quema

### Crear un monedero

Empezar con Botho solo requiere unos pocos pasos:

1. **Visita la página del Monedero**: haz clic en "Abrir monedero" desde la página de inicio o navega directamente al monedero
2. **Elige "Crear nuevo monedero"**: también puedes importar un monedero existente si tienes una frase de recuperación
3. **Protege tu frase de recuperación**: se te mostrará una frase mnemónica de 24 palabras. Anótala en papel y guárdala en un lugar seguro. Esta frase es la **única forma** de recuperar tus fondos si pierdes el acceso a tu dispositivo
4. **Opcional: Define una contraseña**: añade una contraseña de cifrado para mayor seguridad. Necesitarás esta contraseña cada vez que abras el monedero en este navegador

**Importante:** Nunca compartas tu frase de recuperación con nadie. Cualquiera que tenga estas palabras puede acceder a tus fondos. Nunca la guardes de forma digital (sin capturas de pantalla, sin almacenamiento en la nube, sin gestores de contraseñas).

### Entender la dirección de tu monedero

La dirección de tu monedero se parece a esto: `botho://1/4nuKn2U5qsRk3vD...` (unos 90 caracteres en total)

Este formato de dirección incluye:
- **Identificador de protocolo** (`botho://` en mainnet, `tbotho://` en testnet): los distintos prefijos evitan envíos accidentales entre redes
- **Versión de dirección** (`1/`, la versión de dirección actual)
- **Claves públicas**: tu clave de visualización y tu clave de gasto, codificadas juntas en base58

No existe un tipo de dirección a prueba de cuántica separado: el antiguo nivel de direcciones cuánticas `1q/` fue retirado (ADR 0006). La privacidad poscuántica del destinatario es universal por diseño: las claves ML-KEM se derivan de tu dirección estándar, de modo que todas las direcciones mantienen el mismo formato corto.

Puedes compartir esta dirección con total tranquilidad con cualquiera que quiera enviarte fondos. Gracias a las direcciones sigilosas, cada transacción entrante se enviará a una dirección derivada única que solo tú puedes gastar.

### Recibir tu primer pago

Cuando alguien te envía BTH:

1. Usa tu dirección pública para derivar una dirección única de un solo uso
2. La transacción se difunde a la red y se incluye en un bloque
3. Tu monedero escanea los bloques nuevos y detecta los pagos dirigidos a ti
4. Los fondos aparecen en tu saldo, normalmente en un bloque: el tiempo de bloque se adapta a la carga de la red, desde 3 segundos con tráfico muy alto hasta 40 segundos cuando la red está inactiva

### Enviar pagos

Para enviar BTH a otra persona:

1. Haz clic en el botón **Enviar** de tu monedero
2. Introduce la dirección Botho del destinatario
3. Introduce el importe a enviar
4. Revisa los detalles de la transacción, incluida la comisión
5. Confirma la transacción

Las transacciones son definitivas una vez confirmadas: en Botho no hay contracargos ni reversiones.

### Comisiones de transacción

Cada transacción de Botho requiere una pequeña comisión. Estas comisiones cumplen tres propósitos:

1. **Prevención de spam**: las comisiones encarecen inundar la red con transacciones basura
2. **Tributación progresiva**: las comisiones escalan de 1× a 6× según la riqueza del clúster, desincentivando la concentración sin habilitar ataques Sybil
3. **Redistribución y deflación**: el 80 % de cada comisión se redistribuye a los poseedores mediante una lotería; el 20 % restante se quema permanentemente

Las comisiones se basan en el tamaño, no en el importe: `fee = tarifa por byte × tamaño de la transacción × factor de clúster (1×–6×) × penalización por salida`. Consulta las secciones Etiquetas de clúster y Tokenómica para más detalles.

### Buenas prácticas de seguridad

- **Haz una copia de tu frase de recuperación** en papel, guardada en un lugar seguro
- **Usa una contraseña** para cifrar tu monedero en el navegador
- **Considera ejecutar tu propio nodo** para lograr la máxima privacidad
- **Verifica las direcciones con cuidado** antes de enviar fondos: las transacciones no se pueden revertir
