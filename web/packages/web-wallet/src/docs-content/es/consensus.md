## Protocolo de Consenso Stellar

Botho usa el **Protocolo de Consenso Stellar (SCP)** para el consenso distribuido. SCP es un protocolo de acuerdo bizantino federado que proporciona firmeza rápida, eficiencia energética y confianza flexible, sin sacrificar la descentralización.

### ¿Por qué no consenso por prueba de trabajo?

El consenso por prueba de trabajo (PoW), como se usa en Bitcoin, tiene inconvenientes importantes:

- **Desperdicio de energía**: la PoW consume deliberadamente enormes cantidades de electricidad como mecanismo de seguridad
- **Firmeza lenta**: las transacciones de Bitcoin no son realmente definitivas hasta pasada una hora o más
- **Presión de centralización**: las economías de escala de la minería empujan hacia operaciones industriales
- **Ataques del 51 %**: si un atacante controla la mayoría del poder de cómputo, puede reescribir la historia

**Botho aún usa prueba de trabajo, pero solo para la emisión de monedas, nunca para el consenso.** Los bloques se acuñan mediante minería RandomX igualitaria en CPU, que decide *quién gana la recompensa de bloque*. Que ese bloque sea *aceptado* lo decide enteramente SCP. Este desacoplamiento significa que un atacante con la mayoría del poder de cómputo puede ganar más que los demás, pero no puede reescribir la historia ni censurar transacciones; y como el poder de cómputo no compra seguridad, no hay presión de carrera armamentística hacia un consumo energético a escala de Bitcoin.

### ¿Por qué no prueba de participación?

La prueba de participación (PoS) mejora el uso de energía pero introduce sus propios problemas:

- **Nada en juego (nothing-at-stake)**: los validadores pueden votar de forma barata en múltiples bifurcaciones de la cadena
- **Concentración de riqueza**: los ricos se hacen más ricos con las recompensas de staking
- **Ataques de largo alcance**: las claves antiguas pueden potencialmente reescribir la historia
- **Complejidad**: los sistemas PoS requieren una intrincada lógica de slashing y selección de validadores

### Cómo funciona SCP

SCP adopta un enfoque fundamentalmente distinto basado en la **votación federada**:

**Rebanadas de quórum:** Cada nodo de la red define su propia "rebanada de quórum": un conjunto de otros nodos en los que confía. Un nodo solo aceptará una afirmación como definitiva cuando su rebanada de quórum esté de acuerdo.

**Intersección de quórum:** La red es segura mientras todas las rebanadas de quórum compartan algunos nodos en común. Esto garantiza que dos afirmaciones en conflicto no puedan alcanzar ambas el consenso.

**Votación federada:** El consenso avanza a través de una serie de rondas de votación:

1. **Nominar**: los nodos proponen valores candidatos para el siguiente bloque
2. **Preparar**: los nodos votan para preparar un valor específico
3. **Confirmar**: los nodos votan para confirmar el valor preparado
4. **Externalizar**: una vez confirmado, el valor es definitivo

**Idea clave:** A diferencia de la PoW, donde confías en "la cadena más larga", en SCP eliges explícitamente en qué nodos confiar. Esto hace que el modelo de confianza sea transparente y auditable.

### Propiedades de SCP

**Control descentralizado:** Ninguna autoridad central determina el consenso. Cada nodo elige de forma independiente su rebanada de quórum según su propia evaluación de fiabilidad.

**Baja latencia:** Las transacciones alcanzan la firmeza en segundos (normalmente de 3 a 5 segundos en condiciones normales), frente a los minutos u horas de los sistemas PoW.

**Confianza flexible:** Los participantes pueden elegir distintas configuraciones de quórum según sus necesidades. Algunos pueden confiar en instituciones consolidadas; otros pueden confiar en un conjunto de expertos técnicos.

**Seguridad asintótica:** A medida que la red crece y las rebanadas de quórum se interconectan más, el sistema se vuelve más resiliente ante fallos bizantinos.

**Eficiencia energética:** Los nodos SCP solo necesitan intercambiar mensajes y verificar firmas: sin rompecabezas computacionales, sin desperdicio de energía.

### Seguridad frente a vivacidad

SCP prioriza la **seguridad** sobre la **vivacidad**:

- **Seguridad:** La red nunca confirmará transacciones en conflicto
- **Vivacidad:** La red debería, con el tiempo, avanzar

Si la estructura de quórum se ve interrumpida (demasiados nodos se desconectan), SCP se detendrá en lugar de arriesgarse a confirmar transacciones en conflicto. Este es el compromiso correcto para un sistema monetario: es mejor pausar que permitir el robo de fondos.

### Configuración de quórum en Botho

La red Botho comienza con un quórum de arranque centrado en los nodos semilla de la fundación. Con el tiempo, a medida que se unen más nodos independientes, la estructura de quórum se volverá cada vez más descentralizada.

Los operadores de nodos pueden personalizar su rebanada de quórum para confiar en:
- Los nodos semilla de la fundación (por defecto)
- Otros nodos conocidos de la comunidad
- Nodos operados por casas de cambio o negocios en los que confíen
- Cualquier combinación de lo anterior

La salud de la red depende de una intersección de quórum suficiente. El explorador de Botho muestra la topología de quórum en tiempo real para ayudar a los operadores a tomar decisiones informadas.
