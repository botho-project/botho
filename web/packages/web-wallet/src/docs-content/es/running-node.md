## Ejecutar un nodo Botho

Ejecutar tu propio nodo Botho te ofrece el máximo nivel de privacidad y ayuda a fortalecer la red. Cuando ejecutas un nodo, tu monedero se conecta directamente a la cadena de bloques sin depender de servidores de terceros.

### ¿Por qué ejecutar un nodo?

**Privacidad:** Cuando usas un monedero ligero o un monedero web, estás confiando en que un servidor no registre tus direcciones ni tus transacciones. Ejecutar tu propio nodo hace que la actividad de tu monedero se quede en tu máquina.

**Verificación:** Tu nodo valida de forma independiente cada transacción y cada bloque. No tienes que confiar en las afirmaciones de nadie sobre el estado de la red.

**Salud de la red:** Más nodos hacen la red más resiliente. Tu nodo retransmite transacciones y bloques, ayudando a que la red funcione.

**Ventaja de acuñación:** Ejecutar tu propio nodo te da menor latencia en la competición de acuñación. Los nodos que reciben nuevos bloques más rápido pueden empezar a trabajar antes en el siguiente bloque, aumentando sus posibilidades de ganar recompensas de acuñación.

**Participación:** Si quieres acuñar nuevos bloques o participar en el consenso, necesitas un nodo completo.

### Requisitos del sistema

**Requisitos mínimos:**
- 2 núcleos de CPU
- 4 GB de RAM
- 50 GB de almacenamiento SSD
- Conexión a internet de 10 Mbps

**Recomendado:**
- 4+ núcleos de CPU
- 8 GB de RAM
- 100 GB de SSD NVMe
- Conexión a internet de 100 Mbps

La cadena de bloques es actualmente pequeña, pero los requisitos de almacenamiento crecerán con el tiempo.

### Instalación

**Desde el código fuente (recomendado):**

```bash
# Instala Rust si aún no lo tienes
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clona el repositorio
git clone https://github.com/botho-project/botho.git
cd botho

# Compila en modo release
cargo build --release

# El binario está en ./target/release/botho
```

**Configuración inicial:**

```bash
# Inicializa un monedero y una configuración nuevos
./target/release/botho init

# Esto hará lo siguiente:
# - Generar una frase de recuperación de 24 palabras
# - Crear tu configuración y tu monedero en ~/.botho/

# Variantes:
#   botho init --recover   # restaura un monedero desde un mnemónico existente
#   botho init --relay     # configuración de nodo relay/semilla sin monedero
```

**IMPORTANTE:** ¡Anota tu frase de recuperación y guárdala de forma segura!

### Ejecutar el nodo

**Operación básica:**

```bash
# Inicia el nodo y sincroniza con la red
./target/release/botho run
```

**Con acuñación habilitada:**

```bash
# Inicia el nodo y participa en la producción de bloques
./target/release/botho run --mint
```

### Referencia de comandos de la CLI

**Comandos del monedero:**

| Comando | Descripción |
|---------|-------------|
| `botho init` | Crea un nuevo monedero con un mnemónico de 24 palabras |
| `botho balance` | Muestra el saldo actual de tu monedero |
| `botho address` | Muestra tu dirección de recepción (`--save` la escribe en un archivo) |
| `botho send <address> <amount>` | Envía BTH (importe en BTH; `--memo` para adjuntar una nota cifrada) |

Todos los envíos usan firmas de anillo CLSAG: la privacidad del remitente está activa por defecto, no es un flag.

**Comandos del nodo:**

| Comando | Descripción |
|---------|-------------|
| `botho run` | Inicia el nodo y sincroniza con la red |
| `botho run --mint` | Inicia con la acuñación habilitada (`--mint-threads N` para limitar el uso de CPU) |
| `botho status` | Muestra el estado de sincronización del nodo y el número de pares |
| `botho snapshot` | Gestiona las instantáneas de UTXO para una sincronización inicial rápida |

### Configuración

El archivo de configuración se encuentra en `~/.botho/`. Todos los puertos tienen valores por defecto específicos de la red, así que una configuración mínima funciona sin más:

```toml
# "mainnet" o "testnet"
network_type = "testnet"

[network]
# Por defecto: gossip 7100 (mainnet) / 17100 (testnet)
#              RPC    7101 (mainnet) / 17101 (testnet)
#              metrics 9090 (mainnet) / 19090 (testnet), 0 lo desactiva
# gossip_port = 17100
# rpc_port = 17101
# metrics_port = 19090

# Pares de arranque explícitos opcionales (formato multiaddr).
# Si no se define, los pares se descubren mediante registros TXT de DNS
# (seeds.botho.io / seeds.testnet.botho.io).
# bootstrap_peers = ["/dns4/eu.seed.botho.io/tcp/7100/p2p/<peer-id>"]

[minting]
enabled = false
threads = 0   # 0 = usar todos los núcleos de CPU
```

### Configuración del cortafuegos

Si quieres que tu nodo acepte conexiones entrantes (recomendado):

```bash
# Permite el tráfico de gossip P2P (17100 en testnet, 7100 en mainnet)
sudo ufw allow 17100/tcp

# Opcional: permite el acceso RPC (solo si se necesita externamente)
# sudo ufw allow 17101/tcp
```

### Resolución de problemas

**El nodo no sincroniza:**
- Comprueba tu conexión a internet
- Verifica que el cortafuegos permita conexiones salientes en el puerto de gossip
- Como último recurso, borra la base de datos de la cadena en `~/.botho/` y vuelve a sincronizar (solo en testnet: esto reescanea desde el génesis)

**Uso elevado de memoria:**
- Reduce los hilos de acuñación (RandomX mantiene un gran conjunto de datos en memoria)
- Considera añadir espacio de intercambio (swap) si la RAM es limitada

**No se puede conectar con los pares:**
- Asegúrate de que tu puerto de gossip (17100 testnet / 7100 mainnet) esté abierto para conexiones entrantes
- Comprueba si estás detrás de un NAT estricto
