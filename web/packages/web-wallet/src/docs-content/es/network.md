## Información de la red

Esta página ofrece detalles técnicos sobre la red Botho, incluida la información de conexión, los parámetros de red y el modelo de seguridad.

### Estado de la red

La red Botho se encuentra actualmente en fase de **testnet**. Esto significa que:

- Las monedas no tienen valor monetario
- La red puede reiniciarse durante el desarrollo
- Las funciones aún se están probando y refinando
- Se agradecen los informes de errores y los comentarios

El lanzamiento de la mainnet de producción se anunciará cuando la red sea estable.

### Conectarse a la red

**Descubrimiento de semillas:**

Los pares de arranque se descubren mediante registros TXT de DNS en lugar de una lista codificada:

| Red | Dominio semilla de DNS |
|---------|-----------------|
| Mainnet | seeds.botho.io |
| Testnet | seeds.testnet.botho.io |

Cuando tu nodo arranca, resuelve el dominio semilla para conocer los pares de arranque (también puedes fijar `bootstrap_peers` explícitos en la configuración). Tras el descubrimiento inicial, tu nodo mantiene conexiones con múltiples pares para lograr redundancia.

**Descubrimiento de pares:**

Botho usa libp2p para la red, que admite múltiples mecanismos de descubrimiento:

- **Nodos de arranque**: nodos semilla conocidos para la conexión inicial
- **mDNS**: descubrimiento en la red local para desarrollo
- **DHT Kademlia**: descubrimiento distribuido de pares
- **Gossipsub**: propagación de mensajes basada en temas

### Parámetros de red

**Producción de bloques:**

| Parámetro | Valor | Descripción |
|-----------|-------|-------------|
| Tiempo de bloque | 3–40 segundos (adaptativo a la carga) | Bloques rápidos con tráfico alto (3 s solo con 20+ tx/s), bloques lentos cuando está inactiva |
| Tamaño máximo de bloque | 20 MB | Tamaño máximo del bloque serializado |
| Máximo de transacciones por bloque | 5000 | Límite de recuento de transacciones |

**Límites de transacción:**

| Parámetro | Valor | Descripción |
|-----------|-------|-------------|
| Máximo de entradas | 16 | Máximo de entradas por transacción |
| Máximo de salidas | 16 | Máximo de salidas por transacción |
| Tamaño del anillo | 20 | Número de miembros en la firma de anillo CLSAG |
| Tamaño máximo de tx | 100 KB | Tamaño máximo de la transacción serializada |

**Comisiones:**

| Parámetro | Valor | Descripción |
|-----------|-------|-------------|
| Fórmula de comisión | tarifa por byte × tamaño × factor de clúster × penalización por salida | Basada en el tamaño, progresiva según la riqueza |
| Factor de clúster | 1x–6x | Multiplicador progresivo según la procedencia de la moneda |
| Penalización por salida | cuadrática, limitada a 100x | Anti-farming de UTXO |
| Destino de la comisión | 80 % lotería / 20 % quemado | Redistribución más presión deflacionaria |

### Referencia de puertos

Los valores por defecto difieren según la red (mainnet / testnet); todos son configurables:

| Puerto (mainnet / testnet) | Protocolo | Propósito |
|--------------------------|----------|---------|
| 7100 / 17100 | TCP | Gossip P2P (libp2p) |
| 7101 / 17101 | HTTP + WebSocket | API JSON-RPC y actualizaciones en tiempo real |
| 9090 / 19090 | HTTP | Métricas Prometheus |

### Seguridad de la red

**Resistencia a Sybil:**

La red resiste los ataques Sybil mediante:
- Consenso basado en quórum (SCP)
- Puntuación de reputación de los pares
- Requisitos de recursos para la acuñación de bloques

**Protección contra eclipse:**

Los nodos se protegen contra ataques de eclipse mediante:
- El mantenimiento de conexiones diversas con los pares
- La preferencia por pares con un historial establecido
- La rotación regular de pares
- Múltiples métodos independientes de descubrimiento de pares

### Cómo participar

**Para desarrolladores:**
- Código fuente: [github.com/botho-project/botho](https://github.com/botho-project/botho)
- Informa de errores mediante los issues de GitHub
- Se agradecen las contribuciones (ver CONTRIBUTING.md)

**Para operadores de nodos:**
- Ejecuta un nodo para fortalecer la red
- Habilita la acuñación si tienes un tiempo de actividad fiable
- Monitorea la intersección de quórum de tu nodo

**Para usuarios:**
- Prueba el monedero e informa de los problemas
- Aporta comentarios sobre la experiencia de usuario
- Ayuda con la documentación y las traducciones
