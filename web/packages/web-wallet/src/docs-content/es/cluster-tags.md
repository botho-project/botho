## Etiquetas de clúster

Las etiquetas de clúster son el mecanismo novedoso de Botho para rastrear la procedencia de las monedas sin comprometer la privacidad. Habilitan **comisiones progresivas resistentes a Sybil**, **redistribución basada en lotería** y **firmas de anillo que preservan la privacidad**.

### El problema: gravar la riqueza sin identidad

La tributación progresiva tradicional requiere identidad. En criptomonedas:

- Las **comisiones basadas en el importe** fallan al instante: divide 1M en 1000×1K y paga tarifas más bajas
- La **tributación basada en cuentas** es imposible: cualquiera puede crear direcciones ilimitadas
- El **recuento de transacciones** no funciona: los bots pueden generar actividad artificial

**El reto central:** ¿Cómo gravar la concentración de riqueza cuando no puedes identificar quién posee qué?

### La solución: rastreo de procedencia

En lugar de rastrear *quién* posee las monedas, rastreamos *de dónde* vienen. Cada moneda lleva un recuerdo de su origen.

**Idea clave:** Dividir monedas no cambia de dónde vinieron.

### Cómo funcionan las etiquetas de clúster

**1. Los clústeres nacen en la acuñación**

Cada recompensa de bloque crea un "clúster" único: una identidad para las monedas acuñadas por un acuñador concreto. La recompensa de acuñación lleva una etiqueta del 100 % para ese clúster.

```
Bloque 1000: el acuñador A recibe 50 BTH
  → Etiqueta: {cluster_A: 100%}

Bloque 1001: el acuñador B recibe 50 BTH
  → Etiqueta: {cluster_B: 100%}
```

**2. Las etiquetas se heredan al transferir**

Cuando las monedas se mueven, el UTXO del destinatario hereda las etiquetas del remitente:

```
Acuñador A → Comercio → Cliente
   100%   →   95%    →   90%   (factor de clúster de A)
```

**3. Las etiquetas se mezclan al combinar**

Cuando se gastan varias entradas juntas, las etiquetas de salida son un promedio ponderado por valor:

```
Entrada 1: 70 BTH {cluster_A: 100%}
Entrada 2: 30 BTH {cluster_B: 100%}
─────────────────────────────────
Salida: 100 BTH {cluster_A: 70%, cluster_B: 30%}
```

**4. Las etiquetas decaen al moverse las monedas**

Los monederos estándar reducen los pesos de las etiquetas un 5 % en cada transferencia, repartiendo la atribución por toda la economía: un peso se reduce a la mitad tras unas 13,5 transferencias. Es una regla del monedero, no del consenso. No hay restricción por antigüedad, y la red en sí no aplica ningún decaimiento; solo rechaza las salidas que reclaman *más* peso de clúster que sus entradas. Qué debe garantizar el protocolo sobre la persistencia de las etiquetas es una decisión abierta (#1687).

### Por qué dividir no funciona

Esto es lo que hace especiales a las etiquetas de clúster:

```
Una ballena divide 1 000 000 BTH en 1000 × 1000 BTH

Antes: 1 UTXO con {whale_cluster: 100%}
Después: 1000 UTXO, cada uno con {whale_cluster: 100%}

Tarifa de comisión: sin cambios (basada en la riqueza del clúster, no en el número de UTXO)
```

La "riqueza de origen" de un clúster es el valor total acuñado por ese acuñador: dividir no lo reduce.

### Curva de comisión progresiva

El factor de clúster determina cuánto pagas. Sigue una curva sigmoidal suave en el *logaritmo* de la riqueza del clúster, con su punto medio en 100 000 BTH:

| Riqueza del clúster | Multiplicador de comisión |
|----------------|----------------|
| Clústeres pequeños (≤ ~1K BTH) | ~1x (tarifa base) |
| Clústeres medianos (~100K BTH) | ~3.5x (punto medio de la curva) |
| Clústeres ballena (≥ ~10M BTH) | ~6x (se satura) |

El multiplicador se aplica a una comisión basada en el tamaño (`tarifa por byte × tamaño de la transacción`), de modo que la misma transferencia cuesta a un clúster ballena hasta 6× lo que cuesta a monedas bien circuladas, y ninguna cantidad de división cambia eso.

### Redistribución basada en lotería

El 80 % de todas las comisiones de transacción se redistribuye a los UTXO elegibles mediante una lotería. El 20 % se quema.

**Cómo funciona:**

1. Cada transacción paga una comisión basada en el factor de clúster
2. El 80 % de la comisión se reparte entre 4 ganadores elegidos con aleatoriedad verificable
3. El 20 % se quema permanentemente (deflacionario)

**Cómo se seleccionan los ganadores (ponderado por clúster):**

El peso ganador de un UTXO es su **valor dividido por su factor de clúster**. Esta es la única ponderación progresiva que es invariante a la división:

- Los pesos se basan en el valor, así que dividir una posición en muchos UTXO nunca aumenta el peso total
- El sesgo proviene de la procedencia del clúster, que se hereda a través de las divisiones
- Las monedas bien circuladas (factor bajo) ganan proporcionalmente más; las monedas de clúster ballena ganan menos

Para participar, un UTXO debe tener al menos 720 bloques de antigüedad y valer al menos 1 µBTH.

### Firmas de anillo y privacidad de las etiquetas

Las etiquetas de clúster funcionan sin problemas con las firmas de anillo CLSAG:

**El reto:** Las firmas de anillo ocultan qué entrada es la real entre 20 miembros del anillo. ¿Cómo calculamos la comisión correcta?

**La solución:** validación basada en el centroide

1. Las etiquetas de todos los miembros del anillo son públicamente conocidas
2. La comisión se deriva del *centroide* ponderado por valor de las etiquetas del anillo, con suelos que impiden que los señuelos baratos de fondo arrastren el factor hacia abajo
3. Las etiquetas de salida reclamadas deben tener al menos un 70 % de similitud (similitud coseno) con el centroide del anillo, o la transacción se rechaza

Esto previene la evasión de comisiones: no puedes seleccionar a dedo señuelos de factor bajo para reducir tu comisión, porque las composiciones de anillo inverosímiles no pasan la validación.

### Detalles del mecanismo de decaimiento

Cómo funciona el decaimiento hoy (tal como está implementado):

- **Política del monedero:** los monederos estándar (el monedero del nodo y el monedero CLI) mezclan las etiquetas de las entradas ponderadas por valor y luego reducen el resultado un 5 % en cada transferencia
- **Sin restricción por antigüedad:** el decaimiento se aplica sin importar lo reciente que sea la moneda gastada, de modo que un peso se reduce a la mitad tras unas 13,5 transferencias, por rápidas que sean
- **Consenso:** no aplica ningún decaimiento y solo rechaza la *inflación* de etiquetas (salidas que reclaman más peso de clúster que las entradas). Se aceptan etiquetas de salida menores o vacías, así que un monedero modificado puede eliminar las etiquetas en una sola transacción

**El wash trading (enviarte fondos a ti mismo repetidamente para decaer las etiquetas) no está limitado en frecuencia por el protocolo.** Su coste son las comisiones de transacción y el cargo de demurrage por bajada de clase, no el tiempo. Versiones anteriores de esta documentación describían una restricción de antigüedad de 720 bloques que limitaba el decaimiento a ~12 eventos por día; la red no aplica ese diseño. La persistencia de las etiquetas es una decisión abierta (#1687).

| Escenario | Resultado (monedero estándar) |
|-----------|-------------------------------|
| Una transferencia | 5 % de decaimiento |
| ~14 transferencias (a cualquier velocidad) | ~50 % de decaimiento |
| Retener sin transaccionar | 0 % de decaimiento |

### Consideraciones de privacidad

**Fase 1 (actual):** Las etiquetas son públicas en los UTXO. Esto permite la verificación directa de comisiones pero revela cierta información de procedencia.

**Fase 2 (planificada):** Las etiquetas se ocultarán mediante compromisos de Pedersen con pruebas de conocimiento cero. Los validadores verifican las comisiones correctas sin ver los valores reales de las etiquetas.

### Incentivos económicos

El sistema de etiquetas de clúster crea incentivos alineados:

| Comportamiento | Efecto en las etiquetas | Incentivo |
|----------|----------------|-----------|
| **Circular monedas** | Las etiquetas decaen y se mezclan | Comisiones más bajas, mayor peso en la lotería |
| **Acumular riqueza** | Las etiquetas permanecen concentradas | Comisiones más altas, menor peso en la lotería, demurrage |
| **Dividir en muchos UTXO** | Etiquetas sin cambios | Sin beneficio: las comisiones y el peso en la lotería se basan en la procedencia y el valor |

### Parámetros técnicos

| Parámetro | Valor | Propósito |
|-----------|-------|---------|
| Tasa de decaimiento | 5 % por transferencia (política del monedero; el consenso no aplica ninguno) | Difusión gradual de etiquetas |
| Antigüedad mínima del UTXO (lotería) | 720 bloques | Elegibilidad para la lotería |
| Valor mínimo del UTXO (lotería) | 1 µBTH | Exclusión de polvo (dust) |
| Rango del factor de clúster | 1x–6x (punto medio 3.5x en 100K BTH) | Comisiones progresivas |
| Tamaño del anillo | 20 | Conjunto de privacidad para la propagación de etiquetas |
| Ganadores de la lotería | 4 por sorteo | Granularidad de la redistribución |
| Tasa de quema | 20 % de las comisiones | Presión deflacionaria |
| Tasa del fondo | 80 % de las comisiones | Cantidad de redistribución |
| Demurrage | 2 %/año al factor máximo (ver Tokenómica) | Alcanza la riqueza concentrada e inactiva |

### Resumen

Las etiquetas de clúster resuelven el problema de la "tributación progresiva resistente a Sybil" que aqueja a las criptomonedas:

1. **Rastrean procedencia, no identidad**: las monedas recuerdan su origen
2. **Resisten los ataques de división**: la riqueza del clúster se fija en la acuñación
3. **Habilitan comisiones progresivas**: los clústeres ricos pagan más
4. **Impulsan una redistribución justa**: el peso en la lotería se inclina hacia las monedas bien circuladas
5. **Preservan la privacidad**: funcionan con firmas de anillo
6. **Fomentan la circulación**: las etiquetas decaen a través del comercio

Esto convierte a Botho en la primera criptomoneda con un mecanismo creíble para comisiones basadas en la riqueza que no se pueden evadir de forma trivial.
