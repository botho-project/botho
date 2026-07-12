## Tokenómica

Botho (BTH) usa un modelo de emisión en dos fases diseñado para la sostenibilidad a largo plazo: una fase inicial de distribución con halvings, seguida de una emisión de cola perpetua orientada a una inflación estable.

### Visión general

| Parámetro | Valor |
|-----------|-------|
| Símbolo del token | BTH |
| Unidad más pequeña | picocrédito (10⁻¹² BTH) |
| Preminado | Ninguno (100 % minado) |
| Oferta de la Fase 1 | ~611 millones de BTH |
| Tiempo de bloque | 3–40 segundos (adaptativo a la carga; 5 s de referencia monetaria) |

### Sistema de unidades

BTH usa una precisión de 12 decimales. El picocrédito es la única unidad base: cada importe en la red (saldos, comisiones, riqueza de clúster) se denomina en picocréditos, y el formateo a BTH ocurre solo en la capa de presentación:

- **1 picocrédito** = 0.000000000001 BTH (unidad más pequeña)
- **1 microBTH (µBTH)** = 1 000 000 picocréditos = 0.000001 BTH
- **1 miliBTH (mBTH)** = 1 000 000 000 picocréditos = 0.001 BTH
- **1 BTH** = 1 000 000 000 000 picocréditos

---

## Calendario de emisión

Todos los cálculos monetarios asumen el tiempo de bloque de 5 segundos con carga alta (los bloques reales van de 3 a 40 s, bajando a 3 s solo con carga muy alta, 20+ tx/s). Cuando la red está inactiva y los bloques se ralentizan (hasta 40 s), la emisión se estira proporcionalmente: un amortiguador natural: una red ocupada emite según el calendario completo, una inactiva emite menos.

### Fase 1: periodo de halving (~5 años a plena carga)

La recompensa de acuñación comienza en 50 BTH y se reduce a la mitad cada **6 307 200 bloques** (un año de bloques de 5 segundos). Tras cinco épocas de halving, la Fase 1 ha distribuido **611 010 000 BTH**:

| Época | Recompensa de acuñación | Oferta acumulada |
|-------|----------------|-------------------|
| 1 | 50 BTH | ~315.4M BTH |
| 2 | 25 BTH | ~473.0M BTH |
| 3 | 12.5 BTH | ~552.0M BTH |
| 4 | 6.25 BTH | ~591.3M BTH |
| 5 | 3.125 BTH | ~611.0M BTH |

(Este es el calendario canónico ratificado en el issue #351, fijado por pruebas de regresión en el nodo.)

### Fase 2: emisión de cola

Tras la Fase 1, Botho pasa a una emisión de cola perpetua orientada a una **inflación neta anual del 2 %** (emisión bruta menos las quemas de comisiones), con la dificultad ajustándose para alcanzar el objetivo.

**¿Por qué emisión de cola?**

- **Presupuesto de seguridad**: garantiza que los acuñadores siempre tengan incentivo para asegurar la red
- **Reemplazo de monedas perdidas**: compensa las monedas perdidas por claves olvidadas
- **Política monetaria predecible**: el 2 % está por debajo de la inflación fiat típica

Con la oferta de ~611M de BTH de la Fase 1 y a plena carga, el 2 % equivale a aproximadamente **2 BTH por bloque**, creciendo lentamente con la oferta.

---

## Estructura de comisiones

### Comisiones de transacción

Cada comisión se divide en dos: el **80 % se redistribuye** a los poseedores mediante la lotería inclinada por clúster, y el **20 % se quema**, creando una presión deflacionaria que compensa la emisión de cola.

```
fee = tarifa por byte × tamaño de la transacción × factor de clúster × penalización por salida + comisiones de memo
```

| Parámetro | Valor |
|-----------|-------|
| Base de la comisión | Tamaño de la transacción (bytes), no el importe |
| Factor de clúster | Multiplicador progresivo 1x–6x |
| Penalización por salida | Cuadrática en el número de salidas, limitada a 100x |
| Comisión por memo | Plana por cada memo cifrado |
| Destino de la comisión | 80 % al fondo de lotería / 20 % quemado |
| Prioridad | Comisiones más altas = confirmación más rápida |

### Comisiones progresivas basadas en clúster

Botho implementa un novedoso **sistema de comisiones progresivas** que grava la concentración de riqueza sin habilitar ataques Sybil.

**La innovación central:** En lugar de gravar según el importe de la transacción (fácil de manipular dividiendo), las comisiones se basan en la *procedencia* de la moneda: de dónde vinieron originalmente las monedas.

| Parámetro | Valor |
|-----------|-------|
| Rango del factor de clúster | multiplicador de 1x a 6x |
| Forma de la curva | Sigmoide en log(riqueza del clúster), punto medio 3.5x en 100K BTH |
| Decaimiento de etiquetas | 5 % por salto elegible |

**Por qué es resistente a Sybil:** Dividir monedas no cambia su origen. Las monedas de una ballena llevan la misma etiqueta de clúster tanto si se guardan en 1 UTXO como en 1000.

### Demurrage

Las comisiones de transacción son un impuesto al consumo: no pueden tocar la riqueza que nunca se mueve. El demurrage cierra esa brecha: un **cargo por retención sobre las monedas de clúster rico, pagado cuando finalmente se gastan**.

| Parámetro | Valor |
|-----------|-------|
| Tasa | 2 % anual al factor de clúster máximo (6x) |
| Escalado | Proporcional a (factor − 1): las monedas de factor 1 pagan **cero** |
| Arranque | Deshabilitado durante la primera época de halving |
| Ingresos | Fluyen al fondo de redistribución por lotería |

Las monedas de uso cotidiano nunca pagan demurrage; solo afecta a la riqueza concentrada e inactiva. Mover las monedas no lo evita: gastar paga primero el cargo acumulado, así que el total pagado durante cualquier periodo de retención es el mismo sin importar cuántas veces te autotransfieras (y cada transferencia añade comisiones por encima).

> **Consulta la sección [Etiquetas de clúster](#cluster-tags)** para una explicación completa de cómo el rastreo de procedencia, las comisiones progresivas, la redistribución por lotería y la privacidad de las firmas de anillo funcionan juntos.

---

## Proyecciones de oferta

### Crecimiento a largo plazo

A plena carga sostenida (bloques de 5 segundos; más lentos cuando la red está inactiva):

| Época | Oferta aproximada | Inflación anual |
|-------|-------------------|------------------|
| 1 | ~315M BTH | Alta (inicial) |
| 3 | ~552M BTH | ~17 % |
| 5 | ~611M BTH | ~3 % |
| Cola (perpetua) | +2 %/año neto de quemas | 2 % |

---

## Filosofía del diseño económico

### ¿Por qué no hay preminado?

- **Distribución justa**: todos empiezan iguales; los primeros acuñadores asumen riesgo
- **Credibilidad**: sin ventaja para iniciados ni enriquecimiento del fundador
- **Descentralización**: sin tenencias concentradas desde el primer día

### ¿Por qué dividir las comisiones 80/20?

- **Redistribución**: el 80 % regresa a los poseedores mediante la lotería inclinada por clúster, favoreciendo a las monedas bien circuladas
- **Presión deflacionaria**: la quema del 20 % compensa la emisión de cola
- **Predecible**: inflación neta = emisión bruta − quemas

### ¿Por qué comisiones progresivas por clúster?

- **Reducir la concentración**: los clústeres ricos pagan más
- **Resistentes a Sybil**: no se pueden evitar dividiendo cuentas
- **Fomentan la circulación**: mover monedas difunde las etiquetas, reduciendo las comisiones
- **Compatibles con la privacidad**: funcionan con firmas de anillo y direcciones sigilosas

> **En profundidad:** Consulta la documentación de [Etiquetas de clúster](#cluster-tags) para la explicación técnica completa.
