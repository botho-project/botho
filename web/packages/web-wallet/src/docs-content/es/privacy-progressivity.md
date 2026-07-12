## Cómo Botho logra a la vez privacidad y progresividad

Este es el logro más sorprendente de Botho: **la privacidad y la economía progresiva funcionando juntas, no en conflicto.**

La sabiduría convencional dice que estos objetivos son incompatibles. Para gravar más a los ricos, necesitas saber quién es rico. Pero saber quién es rico significa rastrear identidad y saldos, lo contrario de la privacidad.

Botho demuestra que esa disyuntiva es falsa. Así lo hace.

### La aparente imposibilidad

Considera lo que la "tributación progresiva" requiere tradicionalmente:

1. **Identificar al contribuyente**: vincular las transacciones a una persona
2. **Rastrear su riqueza**: saber cuánto posee
3. **Aplicar tarifas graduadas**: cobrar más a quienes tienen más

Cada paso viola la privacidad. Si sabes que Alice tiene 1M de monedas, ya has destruido su privacidad financiera.

Esto crea lo que parece un conflicto fundamental:

| Objetivo | Requiere |
|------|----------|
| **Privacidad** | Ocultar quién posee qué |
| **Progresividad** | Saber quién posee qué |

La mayoría de los proyectos aceptan esta disyuntiva y eligen entre:
- **Privacidad total** (Monero, Zcash): sin comisiones progresivas posibles
- **Transparencia total** (Bitcoin, Ethereum): comisiones progresivas posibles pero sin privacidad

Botho toma una tercera vía.

### La idea clave: procedencia, no identidad

El avance está en darse cuenta de que la tributación progresiva no *requiere* conocer la identidad ni la riqueza total. Requiere **correlacionar las comisiones con el comportamiento económico**.

En lugar de preguntar "¿Quién posee esta moneda y cuán rico es?", Botho pregunta:

> "¿De dónde vino esta moneda y cuánto ha circulado?"

Esta pregunta tiene una propiedad sorprendente: **es respondible en la cadena sin vincularla a una identidad.**

### Qué rastreamos: la proximidad a la acuñación

Cada moneda en Botho lleva una "etiqueta de clúster", un recuerdo de qué evento de acuñación la creó. Esto no rastrea al *propietario*, sino el *origen*.

| Estado de la moneda | Etiquetas | Nivel de comisión |
|------------|------|-----------|
| Recién acuñada | `{cluster_A: 100%}` | Alto |
| Bien negociada | `{cluster_A: 5%, cluster_B: 15%, ...}` | Bajo |

Las propiedades clave:

| Propiedad | Efecto |
|----------|--------|
| **Etiquetas concentradas** | Recién acuñada, aún sin circular → Comisiones altas |
| **Etiquetas diversificadas** | Negociada por muchas manos → Comisiones bajas |
| **Resistente a la división** | Dividir preserva la concentración de etiquetas |
| **Resistente al decaimiento** | Solo el comercio real reduce las etiquetas |

### Por qué esto es progresivo

Aquí es donde se pone interesante. **La proximidad a la acuñación se correlaciona con la concentración de riqueza** de formas predecibles:

**Los nuevos acuñadores tienden a ser ricos.** Ganar recompensas de bloque requiere hardware, electricidad y tiempo de actividad fiable. Incluso con la minería RandomX igualitaria en CPU, las monedas nuevas van desproporcionadamente a quienes ya tienen recursos.

**Los negociantes activos tienden a ser comerciantes.** Los pequeños negocios y los usuarios habituales transaccionan con frecuencia, lo que hace que sus monedas se mezclen con monedas de muchas fuentes.

**Los acaparadores mantienen etiquetas concentradas.** Si minas monedas y las retienes sin negociar, tus etiquetas nunca decaen. Sigues pagando comisiones altas.

**El comercio diversifica de forma natural.** Cada transacción legítima mezcla tus etiquetas con las de tu contraparte. La actividad económica reduce automáticamente tu tarifa de comisión.

Esto crea una **correlación de comportamiento**:

| Patrón de comportamiento | Estado de las etiquetas | Nivel de comisión |
|------------------|-----------|-----------|
| Minar y retener (comportamiento de rico) | Concentrado | Alto |
| Comercio activo (comportamiento de comerciante) | Diversificado | Bajo |
| Gasto de usuario habitual | Mixto | Medio→Bajo |
| Ballena acumulando | Concentrado | Alto |

No necesitamos saber *quién* eres. Simplemente observamos *cómo se comportan tus monedas*.

### Qué permanece privado

Esto es crucial: **las etiquetas de clúster revelan procedencia, no identidad.**

| Información | Estado |
|-------------|--------|
| Quién posee un UTXO | **Privado** (firmas de anillo) |
| Quién recibió un pago | **Privado** (direcciones sigilosas) |
| Importe transferido | **Privado** (transacciones confidenciales) |
| Tu saldo total | **Privado** (sin vinculación de cuentas) |
| Qué UTXO gastaste | **Privado** (firmas de anillo) |
| De dónde se originaron las monedas | Público (etiquetas de clúster) |
| Cuán diversificadas están las etiquetas | Público (permite el cálculo de comisiones) |

Revelas *algo* —la historia de la moneda— pero no *quién eres* ni *qué posees*.

### La integración con las firmas de anillo

Las firmas de anillo y las etiquetas de clúster funcionan juntas sin problemas:

**Cómo funcionan las firmas de anillo:** Cuando gastas, demuestras "poseo UNO de estos 20 UTXO" sin revelar cuál. Esto proporciona privacidad del remitente.

**El reto:** Si no sabemos qué UTXO estás gastando, ¿cómo calculamos la comisión correcta?

**La solución: validación basada en el centroide.**

1. Las etiquetas de los 20 miembros del anillo son visibles públicamente
2. La comisión se deriva del **centroide ponderado por valor** de las etiquetas del anillo, con suelos para que los señuelos baratos de fondo no puedan arrastrar el factor hacia abajo
3. Las etiquetas de salida que reclamas deben tener al menos un 70 % de similitud con ese centroide, o los validadores rechazan la transacción

```
Etiquetas de los miembros del anillo → centroide ponderado por valor → factor de clúster
Etiquetas de salida reclamadas: deben ser ≥ 70 % similares al centroide
```

Esto significa que **la privacidad no habilita la evasión de comisiones.** Una entrada real grande domina el centroide de su propio anillo, así que seleccionar a dedo señuelos de factor bajo produce un anillo inverosímil que falla la validación en lugar de un descuento.

### La redistribución por lotería

Las comisiones regresan a la comunidad a través de una lotería inclinada por clúster:

**El 80 % de todas las comisiones** se redistribuye a los UTXO elegibles. **El 20 % se quema** (deflacionario).

**Cómo funciona la selección:**

```
peso = valor ÷ factor de clúster

UTXO bien circulado (factor 1x):  peso completo por BTH
UTXO de clúster ballena (factor 6x):    1/6 del peso por BTH
```

**Las monedas bien circuladas ganan hasta 6× más por BTH que la riqueza concentrada.** Esta es redistribución progresiva sin conocer la identidad de nadie, y como el peso se basa en el valor, dividir una posición en muchos UTXO nunca aumenta su peso total.

**Las restricciones de elegibilidad** previenen el abuso: un UTXO debe tener al menos 720 bloques de antigüedad y valer al menos 1 µBTH para participar.

### Resistencia a ataques

Hemos probado estos mecanismos exhaustivamente:

**Ataque de división:**
```
Atacante: divide 1M BTH en 1000 × 1K BTH
Resultado: cada pieza aún tiene {whale_cluster: 100%}
Reducción de comisión: 0 % (factor de clúster sin cambios)
Veredicto: ataque derrotado
```

**Ataque Sybil (múltiples cuentas):**
```
Atacante: crea 100 direcciones sybil, envía 10K a cada una
Resultado: el UTXO de cada sybil hereda las etiquetas de la ballena
Reducción de comisión: mínima (las etiquetas se propagan)
Veredicto: ataque derrotado
```

**Ataque de aparcamiento (dividir y esperar):**
```
Atacante: divide en 100 UTXO, espera las ganancias de la lotería
Resultado: el peso se basa en el valor: dividir NO gana peso
        El factor de clúster se hereda: el sesgo sigue en tu contra
        Las comisiones cuadráticas por salida hacen que la división cueste hasta 100×
Veredicto: ataque derrotado
```

**Wash trading (autotransferencias):**
```
Atacante: envía a sí mismo rápidamente para decaer las etiquetas
Resultado: la restricción de antigüedad requiere 720 bloques por decaimiento
        Como máximo ~12 decaimientos por día ≈ 46 % de decaimiento diario
        1 semana de wash trading: ~99 % de decaimiento
        Coste: ~84 comisiones de transacción (cada una alimentando la lotería)
Veredicto: caro, lento, detectable
```

### El panorama completo

Botho logra privacidad + progresividad mediante un diseño por capas:

| Capa | Función de privacidad | Función progresiva |
|-------|-----------------|---------------------|
| **Remitente** | Firmas de anillo (1 de 20) | Propagación de etiquetas validada por centroide |
| **Destinatario** | Direcciones sigilosas | — |
| **Importe** | Compromisos de Pedersen | — |
| **Tarifa de comisión** | — | Curva del factor de clúster (1-6×) |
| **Coste de retención** | — | Demurrage sobre monedas inactivas de clúster rico |
| **Redistribución** | Sorteo aleatorio verificable | Pesos inclinados por clúster (valor ÷ factor) |

Cada capa contribuye a ambos objetivos sin conflicto.

### Por qué esto importa

Esto no es solo un logro técnico: cambia lo que es posible:

**Para los usuarios:** Obtienes privacidad financiera Y un sistema económico justo. Sin disyuntiva.

**Para los comerciantes:** Las comisiones bajas premian la actividad económica. Cuanto más negocias, más bajas son tus tarifas.

**Para la red:** La riqueza fluye de forma natural desde las tenencias concentradas hacia las distribuidas. La autocustodia se premia por encima de los servicios de custodia.

**Para el ecosistema:** Hemos demostrado que "privacidad frente a equidad" es una falsa dicotomía. Otros proyectos pueden adoptar estas técnicas.

### Resumen

El mecanismo de privacidad + progresividad de Botho funciona porque:

1. **Rastreamos procedencia, no identidad**: de dónde vinieron las monedas, no quién las posee
2. **La procedencia se correlaciona con el comportamiento de riqueza**: los acuñadores nuevos son ricos, los negociantes activos son comerciantes
3. **Las firmas de anillo preservan la privacidad**: la validación por centroide previene el abuso
4. **La lotería inclinada por clúster es progresiva**: las monedas bien circuladas ganan más por BTH
5. **Los pesos basados en el valor y las comisiones cuadráticas por salida disuaden los ataques**: aparcar y dividir no compensan
6. **El comercio se premia**: las etiquetas decaen mediante el comercio legítimo; la riqueza concentrada e inactiva paga demurrage

El resultado: **La primera criptomoneda donde puedes ser privado Y contribuir con equidad a la red, donde los ricos pagan más sin que nadie sepa quiénes son.**
