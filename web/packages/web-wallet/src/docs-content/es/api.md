## API JSON-RPC

Los nodos de Botho exponen una API JSON-RPC 2.0 en el puerto **7101** (mainnet) o **17101** (testnet) por defecto; se puede cambiar con `rpc_port` en la configuración. Todas las solicitudes usan el formato estándar JSON-RPC 2.0.

### Formato de la solicitud

```json
{
  "jsonrpc": "2.0",
  "method": "METHOD_NAME",
  "params": { ... },
  "id": 1
}
```

---

## Métodos del nodo

### node_getStatus

Obtiene el estado del nodo y la información de sincronización.

**Respuesta (campos seleccionados):**
- `version` - Versión del software del nodo
- `network` - Nombre de la red (p. ej., "botho-testnet")
- `uptimeSeconds` - Tiempo de actividad del nodo en segundos
- `syncStatus` / `syncProgress` / `synced` - Estado de sincronización en vivo
- `chainHeight` - Altura actual de la cadena de bloques
- `tipHash` - Hash del último bloque
- `peerCount` / `scpPeerCount` - Pares conectados / pares que participan en SCP
- `mempoolSize` - Transacciones en el mempool
- `mintingActive` - Si la acuñación está habilitada
- `quorumFaultTolerant` / `quorumDegenerate` - Postura BFT (la tolerancia a fallos requiere ≥ 4 nodos participantes)

La respuesta completa también incluye información de compilación, el progreso de las ranuras de SCP, el estado de la puerta de quórum y campos de salud del minero para monitoreo.

---

## Métodos de la cadena

### getChainInfo

Obtiene información de la cadena de bloques.

**Respuesta:**
- `height` - Altura de bloque actual
- `tipHash` - Hash del bloque de la punta
- `difficulty` - Dificultad de minería actual
- `totalMined` - Total de monedas minadas (en picocréditos, como cadena)
- `totalFeesBurned` - Comisiones quemadas acumuladas (en picocréditos, como cadena)
- `circulatingSupply` - totalMined menos las quemas (en picocréditos, como cadena)
- `mempoolSize` - Número de transacciones pendientes
- `mempoolFees` - Total de comisiones en el mempool

### getBlockByHeight

Obtiene un bloque por su altura.

**Parámetros:**
- `height` (número) - Altura de bloque

**Respuesta:**
- `height` - Altura de bloque
- `hash` - Hash del bloque
- `prevHash` - Hash del bloque anterior
- `timestamp` - Marca de tiempo del bloque
- `difficulty` - Dificultad del bloque
- `nonce` - Nonce de minería
- `txCount` - Número de transacciones
- `mintingReward` - Importe de la recompensa de acuñación

### getMempoolInfo

Obtiene estadísticas del mempool.

**Respuesta:**
- `size` - Número de transacciones
- `totalFees` - Total de comisiones de todas las transacciones
- `txHashes` - Array de hashes de transacción (hasta 100)

### estimateFee (alias: tx_estimateFee)

Estima la comisión de una transacción.

**Parámetros:**
- `amount` (número) - Importe de la transacción
- `memos` (número) - Número de campos de memo cifrados

**Respuesta:**
- `minimumFee` - Comisión mínima requerida
- `clusterFactor` - Multiplicador progresivo, escalado ×1000 (1000 = 1x, 6000 = 6x)
- `clusterFactorDisplay` - Factor legible para humanos (p. ej., "1.25x")
- `clusterWealth` - Riqueza del clúster de la que se derivó el factor
- `recommendedFee` - Comisión recomendada para prioridad normal
- `highPriorityFee` - Comisión para confirmación de alta prioridad

---

## Métodos del monedero

### chain_getOutputs

Obtiene las salidas de transacción para la sincronización del monedero.

**Parámetros:**
- `start_height` (número) - Altura de bloque inicial
- `end_height` (número) - Altura de bloque final (máximo 100 bloques por solicitud)

**Respuesta:** Array de bloques, cada uno con:
- `height` - Altura de bloque
- `outputs` - Array de salidas con `txHash`, `outputIndex`, `targetKey`, `publicKey`, `amountCommitment`

### wallet_getBalance

Obtiene el saldo del monedero (requiere un monedero local).

**Respuesta:**
- `confirmed` - Saldo confirmado
- `pending` - Saldo pendiente
- `total` - Saldo total
- `utxoCount` - Número de salidas no gastadas

### wallet_getAddress

Obtiene las claves del monedero y la información de la dirección.

**Respuesta:**
- `viewKey` - Clave pública de visualización (hex)
- `spendKey` - Clave pública de gasto (hex)
- `hasWallet` - Si el nodo tiene un monedero configurado

---

## Métodos de transacción

### tx_submit / sendRawTransaction

Envía una transacción firmada.

**Parámetros:**
- `tx_hex` (cadena) - Transacción serializada codificada en hex

**Respuesta:**
- `txHash` - Hash de la transacción

---

## Métodos de acuñación

### minting_getStatus

Obtiene el estado de la acuñación.

**Respuesta:**
- `active` - Si la acuñación está habilitada
- `threads` - Número de hilos de acuñación
- `hashrate` - Tasa de hash actual
- `totalHashes` - Total de hashes calculados
- `blocksFound` - Bloques minados por este nodo
- `currentDifficulty` - Dificultad actual de la red
- `uptimeSeconds` - Tiempo de actividad de la acuñación

---

## Métodos de red

### network_getInfo

Obtiene información de la conexión de red.

**Respuesta:**
- `peerCount` - Número total de pares
- `inboundCount` - Conexiones entrantes
- `outboundCount` - Conexiones salientes
- `bytesSent` - Total de bytes enviados
- `bytesReceived` - Total de bytes recibidos
- `uptimeSeconds` - Tiempo de actividad de la conexión

### network_getPeers

Obtiene la lista de pares conectados.

**Respuesta:**
- `peers` - Array con la información de los pares

---

## Otros métodos

La superficie de la API es mayor que esta página. Métodos adicionales destacados:

| Método | Propósito |
|--------|---------|
| `getBlockByHash` | Obtener un bloque por hash en lugar de por altura |
| `getSupplyInfo` | Detalles de emisión y oferta |
| `tx_get` / `tx_getStatus` | Consultar una transacción / su estado de confirmación |
| `address_validate` | Comprobar si una cadena de dirección está bien formada |
| `fee_getRate` | Tarifa de comisión dinámica actual |
| `cluster_getWealth` / `cluster_getAllWealth` | Consultas de riqueza de clúster (alimentan las vistas del explorador) |
| `chain_areKeyImagesSpent` | Comprobar imágenes de clave para la detección de doble gasto |
| `faucet_getStatus` / `faucet_request` | Grifo (faucet) de testnet |
| `operator_*` | Superficie de confianza del operador (deshabilitada salvo que se configure; ver los runbooks del operador) |
