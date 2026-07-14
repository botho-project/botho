## Funciones de privacidad

La privacidad no es solo una función en Botho: es un principio de diseño fundamental. Cada aspecto del protocolo está diseñado para proteger tu privacidad financiera manteniendo a la vez las propiedades de auditabilidad que necesita un sistema monetario sólido.

### Por qué importa la privacidad

La privacidad financiera es esencial para:

- **Seguridad personal**: la riqueza pública te convierte en objetivo de delincuentes
- **Confidencialidad de negocio**: los competidores no deberían ver tus pagos a proveedores ni tus ingresos
- **Fungibilidad**: el dinero debe ser intercambiable; las monedas "manchadas" crean un sistema de dos niveles
- **Dignidad humana**: tu vida financiera no es asunto de nadie más que tuyo
- **Resistencia a la censura**: cuando todas las transacciones parecen idénticas, no hay base para bloquear pagos concretos. Bitcoin se esfuerza por resolver este problema con diversas técnicas, pero la privacidad por defecto lo resuelve con elegancia: los validadores no pueden discriminar porque no pueden distinguir

### Direcciones sigilosas

Las direcciones sigilosas son la base del modelo de privacidad de Botho. Así funcionan:

**El problema:** En Bitcoin, si publicas una dirección para recibir donaciones, cualquiera puede ver todas las donaciones que has recibido consultando esa dirección en la cadena de bloques.

**La solución:** En Botho, tu dirección pública no es donde realmente se envían los fondos. En su lugar, cada remitente usa tu dirección pública para derivar matemáticamente una dirección única de un solo uso. Solo tú puedes detectar y gastar desde esas direcciones derivadas.

**Detalles técnicos:**

1. Tu monedero tiene un **par de claves de visualización** y un **par de claves de gasto**
2. El remitente genera un valor aleatorio y lo combina con tus claves públicas
3. Esto produce una dirección de un solo uso que parece aleatoria para todos los demás
4. Tu monedero usa tu clave privada de visualización para buscar los pagos dirigidos a ti
5. Para gastar, usas tu clave privada de gasto para firmar la transacción

El resultado: aunque publiques tu dirección públicamente, nadie que observe la cadena de bloques puede determinar cuántos pagos has recibido, cuándo los recibiste ni por cuánto fueron.

### Firmas de anillo (transacciones privadas)

Cuando envías una **transacción privada**, Botho usa **firmas de anillo CLSAG** para ocultar qué monedas concretas estás gastando. Tu transacción hace referencia a 20 posibles entradas (un "anillo"), y la firma demuestra que posees una de ellas sin revelar cuál.

CLSAG (Concise Linkable Spontaneous Anonymous Group) es un esquema eficiente de firma de anillo que ofrece una sólida privacidad del remitente con firmas compactas (~700 bytes por entrada).

Esto rompe el grafo de transacciones que de otro modo permitiría rastrear fondos a través de la cadena de bloques. Un observador ve que *alguien* del anillo gastó *algunas* monedas, pero no puede determinar qué participante ni qué monedas concretas.

> **Nota:** Todas las transferencias de valor usan firmas de anillo. Las transacciones de acuñación (recompensas de bloque) no llevan firma: la preimagen de PoW queda vinculada a las claves públicas del acuñador, de modo que la atribución se basa en hashes y es resistente a la computación cuántica (ADR 0006).

### Importes confidenciales

> **Estado de implementación:** Los importes confidenciales son el diseño ratificado (ADR 0006) y están en desarrollo. En la testnet actual, los importes de las transacciones son públicos. Esta sección describe el diseño objetivo.

En todas las **transacciones privadas** (que incluyen todas las transferencias de valor), los importes se ocultan mediante **compromisos de Pedersen** con pruebas de rango **Bulletproofs**. Estas construcciones criptográficas permiten a la red verificar que las transacciones cuadran (las entradas igualan a las salidas más las comisiones) sin revelar los importes reales.

Los validadores pueden confirmar:
- Que no se crea dinero de la nada
- Que el remitente tiene fondos suficientes
- Que la comisión es al menos el mínimo requerido
- Que todos los importes son positivos (mediante Bulletproofs)

Pero no pueden determinar:
- Cuánto se está transfiriendo
- El saldo total del remitente
- El saldo total del destinatario

> **Nota:** Las transacciones de acuñación (recompensas de bloque) tienen importes públicos para la auditabilidad de la oferta, pero los destinatarios siguen ocultos mediante direcciones sigilosas.

### Criptografía poscuántica

Los ordenadores cuánticos suponen una amenaza futura para los algoritmos criptográficos que hoy protegen la mayoría de las criptomonedas. Botho usa una **arquitectura poscuántica híbrida** que protege los datos más críticos manteniendo las transacciones eficientes.

**Algoritmos utilizados:**

- **ML-KEM-768** (FIPS 203): direcciones sigilosas poscuánticas (la privacidad del destinatario es permanente)
- **Vinculación por preimagen de PoW** (RandomX, basada en hashes): atribución de la acuñación sin firmas (resistente a la computación cuántica)
- **CLSAG**: firmas de anillo clásicas para las transacciones privadas (la privacidad del remitente es efímera)
- **Pedersen + Bulletproofs**: ocultación de importes con seguridad de teoría de la información (a prueba de cuántica; los importes confidenciales están en desarrollo, véase más arriba)

**¿Por qué esta arquitectura?**

La identidad del destinatario queda registrada en la cadena para siempre: un atacante cuántico en 2045 podría vincular destinatarios a partir de transacciones de 2025. ML-KEM protege frente a esta amenaza de "recolectar ahora, descifrar después". La privacidad del remitente, en cambio, es efímera: su valor se degrada con el tiempo a medida que el contexto económico se vuelve histórico. Usar CLSAG clásico mantiene las transacciones pequeñas (~4 KB frente a ~65 KB de las alternativas poscuánticas). ML-DSA-65 (FIPS 204) sigue siendo la familia de firmas futura designada de Botho para la protección frente al robo cuántico, pero hoy no desempeña ningún papel activo en el protocolo.

**Tipos de transacción:**

| Tipo | Destinatario | Importe | Remitente | Caso de uso |
|------|-----------|--------|--------|----------|
| Acuñación | Oculto (ML-KEM) | Público | Conocido (vinculado a PoW) | Recompensas de bloque |
| Privada | Oculto (ML-KEM) | Oculto (en desarrollo) | Oculto (anillo CLSAG=20) | Todas las transferencias (~4 KB) |

Los destinatarios están protegidos frente a los ordenadores cuánticos; la ocultación de importes tendrá seguridad de teoría de la información cuando se implementen los importes confidenciales. La privacidad del remitente usa firmas clásicas eficientes.

### Buenas prácticas de privacidad

Para maximizar tu privacidad al usar Botho:

1. **Ejecuta tu propio nodo**: así evitas revelar tus direcciones a servidores de terceros
2. **Usa una dirección nueva para cada contexto**: aunque las direcciones sigilosas protegen los fondos recibidos, usar direcciones separadas para trabajo y para uso personal añade otra capa
3. **Ten cuidado con los metadatos**: la privacidad en la cadena no sirve de nada si revelas información fuera de la cadena
