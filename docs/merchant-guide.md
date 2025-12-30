# Merchant Acceptance Guide

Accept Botho (BTH) payments for your business.

## Overview

This guide covers how to accept BTH payments for:
- E-commerce websites
- Point-of-sale systems
- Invoicing and billing
- Subscription services

### Why Accept BTH?

| Benefit | Description |
|---------|-------------|
| Low fees | Transaction fees are typically < 0.1% |
| No chargebacks | Transactions are final and irreversible |
| Privacy | Customer payment data stays private |
| Global | Accept payments from anywhere |
| Fast settlement | ~10 minute confirmation |

---

## Quick Start

### Option 1: Self-Hosted Node

Run your own Botho node for full control:

```bash
# Install
cargo build --release
sudo cp target/release/botho /usr/local/bin/

# Initialize wallet
botho init

# Start node
botho run
```

See [Deployment Guide](deployment.md) for production setup.

### Option 2: Payment Processor

Use a third-party payment processor (when available) for:
- Hosted checkout pages
- Automatic fiat conversion
- Simplified integration

---

## Payment Flow

```
┌──────────┐    ┌─────────────┐    ┌──────────┐    ┌──────────┐
│ Customer │───►│ Your Store  │───►│  Botho   │───►│ Confirm  │
│ Checkout │    │ Show Invoice│    │ Payment  │    │ & Fulfill│
└──────────┘    └─────────────┘    └──────────┘    └──────────┘
```

1. **Customer initiates checkout**
2. **Generate payment request** with amount and memo
3. **Customer sends BTH** to your address
4. **Detect payment** via node scanning or WebSocket
5. **Wait for confirmations**
6. **Fulfill order**

---

## Basic Integration

### Generate Payment Request

```python
import hashlib
import time

class PaymentRequest:
    def __init__(self, rpc_url):
        self.rpc_url = rpc_url
        self.pending_payments = {}

    def create_invoice(self, order_id, amount_bth, description=""):
        """Create a payment invoice"""
        # Get wallet address
        address = self.get_wallet_address()

        # Generate unique payment memo
        memo = self.generate_memo(order_id)

        # Store pending payment
        self.pending_payments[memo] = {
            'order_id': order_id,
            'amount': int(amount_bth * 1e12),  # Convert to credits
            'created_at': time.time(),
            'expires_at': time.time() + 3600,  # 1 hour expiry
            'status': 'pending'
        }

        return {
            'address': address,
            'amount_bth': amount_bth,
            'amount_credits': int(amount_bth * 1e12),
            'memo': memo,
            'expires_at': self.pending_payments[memo]['expires_at'],
            'payment_uri': self.build_payment_uri(address, amount_bth, memo)
        }

    def generate_memo(self, order_id):
        """Generate unique payment memo"""
        # 8-character hex identifier
        data = f"{order_id}:{time.time()}"
        return hashlib.sha256(data.encode()).hexdigest()[:8]

    def get_wallet_address(self):
        """Get wallet public keys for payment"""
        result = self.rpc('wallet_getAddress')
        return f"{result['viewKey']}:{result['spendKey']}"

    def build_payment_uri(self, address, amount, memo):
        """Build payment URI for wallets"""
        return f"botho:{address}?amount={amount}&memo={memo}"

    def rpc(self, method, params=None):
        import requests
        response = requests.post(self.rpc_url, json={
            'jsonrpc': '2.0',
            'method': method,
            'params': params or {},
            'id': 1
        })
        return response.json()['result']
```

### Display Payment Page

```html
<!DOCTYPE html>
<html>
<head>
    <title>Pay with Botho</title>
    <style>
        .payment-box {
            max-width: 400px;
            margin: 50px auto;
            padding: 20px;
            border: 2px solid #333;
            border-radius: 8px;
            font-family: system-ui;
        }
        .amount { font-size: 32px; font-weight: bold; }
        .address {
            font-family: monospace;
            font-size: 12px;
            word-break: break-all;
            background: #f5f5f5;
            padding: 10px;
            border-radius: 4px;
        }
        .memo { font-family: monospace; font-size: 18px; }
        .qr { text-align: center; margin: 20px 0; }
        .status { padding: 10px; border-radius: 4px; margin-top: 20px; }
        .status.pending { background: #fff3cd; }
        .status.confirmed { background: #d4edda; }
        .timer { font-size: 14px; color: #666; }
    </style>
</head>
<body>
    <div class="payment-box">
        <h2>Pay with Botho</h2>

        <p>Order: <strong>#{{ order_id }}</strong></p>

        <p class="amount">{{ amount_bth }} BTH</p>

        <div class="qr">
            <!-- QR code for payment URI -->
            <img src="/qr/{{ payment_uri }}" alt="Payment QR Code" width="200">
        </div>

        <p><strong>Send to address:</strong></p>
        <p class="address">{{ address }}</p>

        <p><strong>Include memo:</strong></p>
        <p class="memo">{{ memo }}</p>

        <p class="timer">Expires in: <span id="countdown">60:00</span></p>

        <div class="status pending" id="status">
            Waiting for payment...
        </div>
    </div>

    <script>
        // Poll for payment status
        async function checkPayment() {
            const response = await fetch('/api/payment/{{ payment_id }}/status');
            const data = await response.json();

            if (data.status === 'confirmed') {
                document.getElementById('status').className = 'status confirmed';
                document.getElementById('status').textContent =
                    `Payment confirmed! (${data.confirmations} confirmations)`;

                // Redirect to success page
                setTimeout(() => {
                    window.location.href = '/order/{{ order_id }}/success';
                }, 2000);
            } else if (data.status === 'detected') {
                document.getElementById('status').textContent =
                    `Payment detected, waiting for confirmations (${data.confirmations}/6)...`;
            }
        }

        // Check every 10 seconds
        setInterval(checkPayment, 10000);

        // Countdown timer
        let seconds = 3600;
        setInterval(() => {
            seconds--;
            const mins = Math.floor(seconds / 60);
            const secs = seconds % 60;
            document.getElementById('countdown').textContent =
                `${mins}:${secs.toString().padStart(2, '0')}`;
        }, 1000);
    </script>
</body>
</html>
```

### Detect Payments

```python
import time
import threading

class PaymentMonitor:
    def __init__(self, rpc_url, payment_store, callback):
        self.rpc_url = rpc_url
        self.payment_store = payment_store
        self.callback = callback
        self.last_height = 0
        self.running = False

    def start(self):
        """Start monitoring for payments"""
        self.running = True
        self.thread = threading.Thread(target=self._monitor_loop)
        self.thread.start()

    def stop(self):
        self.running = False
        self.thread.join()

    def _monitor_loop(self):
        while self.running:
            try:
                self._check_new_blocks()
            except Exception as e:
                print(f"Monitor error: {e}")
            time.sleep(10)

    def _check_new_blocks(self):
        status = self.rpc('node_getStatus')
        current_height = status['chainHeight']

        if current_height > self.last_height:
            # Check wallet balance for new transactions
            balance = self.rpc('wallet_getBalance')

            # Scan for outputs matching pending payments
            self._scan_for_payments(self.last_height + 1, current_height)

            self.last_height = current_height

    def _scan_for_payments(self, start, end):
        """Scan blocks for pending payments"""
        outputs = self.rpc('chain_getOutputs', {
            'start_height': start,
            'end_height': end
        })

        for block in outputs:
            for output in block['outputs']:
                # Match against pending payments
                # This requires decrypting memos to find matches
                payment = self._match_payment(output)
                if payment:
                    self._process_payment(payment, block['height'])

    def _match_payment(self, output):
        """Match output to pending payment"""
        # Implementation depends on how memos are used
        # The node's wallet handles output detection
        pass

    def _process_payment(self, payment, height):
        """Process detected payment"""
        current_height = self.rpc('node_getStatus')['chainHeight']
        confirmations = current_height - height

        payment['status'] = 'detected'
        payment['confirmations'] = confirmations
        payment['detected_height'] = height

        if confirmations >= 6:
            payment['status'] = 'confirmed'
            self.callback(payment)

    def rpc(self, method, params=None):
        import requests
        response = requests.post(self.rpc_url, json={
            'jsonrpc': '2.0',
            'method': method,
            'params': params or {},
            'id': 1
        })
        return response.json()['result']
```

### WebSocket Real-Time Updates

```python
import asyncio
import websockets
import json

async def payment_websocket_handler(websocket, path):
    """WebSocket handler for payment status updates"""
    payment_id = path.split('/')[-1]

    async for message in websocket:
        # Client requests status update
        payment = get_payment(payment_id)
        await websocket.send(json.dumps({
            'status': payment['status'],
            'confirmations': payment.get('confirmations', 0),
            'amount': payment['amount']
        }))

async def monitor_and_notify():
    """Monitor blockchain and notify connected clients"""
    async with websockets.connect('ws://localhost:7101/ws') as ws:
        await ws.send(json.dumps({
            'type': 'subscribe',
            'events': ['blocks']
        }))

        async for message in ws:
            msg = json.loads(message)
            if msg.get('type') == 'event' and msg['event'] == 'block':
                # Check pending payments for confirmations
                await update_pending_payments(msg['data']['height'])
```

---

## Confirmation Requirements

| Use Case | Confirmations | Wait Time | Risk |
|----------|---------------|-----------|------|
| Digital goods | 1-3 | 1-3 min | Low value, instant delivery |
| Physical goods | 6 | ~6 min | Standard purchases |
| High-value | 10+ | ~10 min | Expensive items |
| Very high-value | 20+ | ~20 min | Large transactions |

### Zero-Confirmation (Risky)

For very low-value items or trusted customers, you might accept unconfirmed transactions:

```python
def accept_payment(payment, require_confirmations=6):
    if payment['confirmations'] >= require_confirmations:
        return True

    # Zero-conf acceptance criteria
    if payment['amount'] < 10 * 1e12:  # < 10 BTH
        if payment['in_mempool']:
            if not payment['double_spend_detected']:
                return True  # Accept at own risk

    return False
```

**Warning:** Zero-confirmation transactions can be double-spent. Only use for low-value items where the risk is acceptable.

---

## E-Commerce Integration

### WooCommerce Plugin (Conceptual)

```php
<?php
/**
 * Botho Payment Gateway for WooCommerce
 */
class WC_Gateway_Botho extends WC_Payment_Gateway {

    public function __construct() {
        $this->id = 'botho';
        $this->method_title = 'Botho (BTH)';
        $this->method_description = 'Accept BTH cryptocurrency payments';
        $this->has_fields = false;

        $this->init_form_fields();
        $this->init_settings();

        $this->title = $this->get_option('title');
        $this->rpc_url = $this->get_option('rpc_url');

        add_action('woocommerce_update_options_payment_gateways_' . $this->id,
            array($this, 'process_admin_options'));
    }

    public function init_form_fields() {
        $this->form_fields = array(
            'enabled' => array(
                'title' => 'Enable/Disable',
                'type' => 'checkbox',
                'label' => 'Enable Botho Payment',
                'default' => 'yes'
            ),
            'title' => array(
                'title' => 'Title',
                'type' => 'text',
                'default' => 'Pay with Botho (BTH)'
            ),
            'rpc_url' => array(
                'title' => 'Node RPC URL',
                'type' => 'text',
                'default' => 'http://localhost:7101/'
            ),
            'confirmations' => array(
                'title' => 'Required Confirmations',
                'type' => 'number',
                'default' => 6
            )
        );
    }

    public function process_payment($order_id) {
        $order = wc_get_order($order_id);

        // Convert to BTH (example rate - use real pricing)
        $amount_bth = $order->get_total() / $this->get_exchange_rate();

        // Create payment request
        $payment = $this->create_payment_request($order_id, $amount_bth);

        // Store payment details
        update_post_meta($order_id, '_botho_payment', $payment);

        // Mark order as pending
        $order->update_status('pending', 'Awaiting BTH payment');

        return array(
            'result' => 'success',
            'redirect' => $this->get_return_url($order)
        );
    }
}
```

### Shopify Integration (Conceptual)

For Shopify, create a custom payment app:

```javascript
// Shopify payment app handler
app.post('/payment/create', async (req, res) => {
    const { amount, currency, order_id } = req.body;

    // Convert to BTH
    const bthAmount = await convertToBTH(amount, currency);

    // Create invoice
    const invoice = await bothoClient.createInvoice({
        orderId: order_id,
        amount: bthAmount
    });

    res.json({
        redirect_url: `https://yoursite.com/pay/${invoice.id}`
    });
});

app.post('/payment/complete', async (req, res) => {
    const { payment_id } = req.body;

    const payment = await bothoClient.getPayment(payment_id);

    if (payment.status === 'confirmed') {
        res.json({ status: 'completed' });
    } else {
        res.json({ status: 'pending' });
    }
});
```

---

## Point of Sale

### Simple POS Display

```python
from flask import Flask, render_template, jsonify
import qrcode
import io
import base64

app = Flask(__name__)
payment_client = PaymentRequest('http://localhost:7101/')

@app.route('/pos')
def pos_terminal():
    return render_template('pos.html')

@app.route('/pos/create', methods=['POST'])
def create_pos_payment():
    amount = request.json['amount']
    description = request.json.get('description', '')

    # Generate unique order ID
    order_id = f"POS-{int(time.time())}"

    # Create invoice
    invoice = payment_client.create_invoice(order_id, amount, description)

    # Generate QR code
    qr = qrcode.make(invoice['payment_uri'])
    buffer = io.BytesIO()
    qr.save(buffer, format='PNG')
    qr_base64 = base64.b64encode(buffer.getvalue()).decode()

    return jsonify({
        'order_id': order_id,
        'amount': amount,
        'address': invoice['address'],
        'memo': invoice['memo'],
        'qr_code': f"data:image/png;base64,{qr_base64}",
        'expires_at': invoice['expires_at']
    })

@app.route('/pos/status/<order_id>')
def check_pos_payment(order_id):
    payment = payment_client.get_payment_status(order_id)
    return jsonify(payment)
```

### Hardware POS Integration

For dedicated POS hardware:

```python
class POSTerminal:
    def __init__(self, display, printer, rpc_url):
        self.display = display
        self.printer = printer
        self.payment_client = PaymentRequest(rpc_url)

    def start_payment(self, amount, description=""):
        """Start a new payment on the terminal"""
        order_id = self.generate_order_id()
        invoice = self.payment_client.create_invoice(order_id, amount, description)

        # Show on display
        self.display.show_qr(invoice['payment_uri'])
        self.display.show_text(f"Amount: {amount} BTH")
        self.display.show_text(f"Memo: {invoice['memo']}")

        # Wait for payment
        while True:
            status = self.payment_client.get_payment_status(order_id)

            if status['status'] == 'confirmed':
                self.display.show_text("Payment Confirmed!")
                self.print_receipt(order_id, amount, status)
                return True

            if status['status'] == 'expired':
                self.display.show_text("Payment Expired")
                return False

            time.sleep(5)

    def print_receipt(self, order_id, amount, status):
        """Print payment receipt"""
        self.printer.print(f"""
================================
       PAYMENT RECEIPT
================================
Order: {order_id}
Amount: {amount} BTH
Status: CONFIRMED
Confirmations: {status['confirmations']}
TX: {status['tx_hash'][:16]}...
Date: {datetime.now()}
================================
        Thank you!
================================
        """)
```

---

## Invoicing

### Create Invoice

```python
class InvoiceManager:
    def __init__(self, rpc_url, db):
        self.payment_client = PaymentRequest(rpc_url)
        self.db = db

    def create_invoice(self, customer_id, items, due_date=None):
        """Create an invoice for a customer"""
        # Calculate total
        subtotal = sum(item['price'] * item['quantity'] for item in items)
        tax = subtotal * 0.1  # Example 10% tax
        total = subtotal + tax

        # Convert to BTH
        bth_rate = self.get_exchange_rate()
        total_bth = total / bth_rate

        # Generate invoice number
        invoice_number = self.generate_invoice_number()

        # Create payment request
        payment = self.payment_client.create_invoice(
            invoice_number,
            total_bth,
            f"Invoice {invoice_number}"
        )

        # Store invoice
        invoice = {
            'number': invoice_number,
            'customer_id': customer_id,
            'items': items,
            'subtotal': subtotal,
            'tax': tax,
            'total': total,
            'total_bth': total_bth,
            'bth_rate': bth_rate,
            'payment_address': payment['address'],
            'payment_memo': payment['memo'],
            'created_at': time.time(),
            'due_date': due_date or time.time() + 30 * 24 * 3600,
            'status': 'pending'
        }

        self.db.save_invoice(invoice)
        return invoice

    def send_invoice_email(self, invoice, customer_email):
        """Send invoice to customer via email"""
        # Generate PDF
        pdf = self.generate_invoice_pdf(invoice)

        # Send email with payment instructions
        send_email(
            to=customer_email,
            subject=f"Invoice {invoice['number']}",
            body=f"""
Invoice #{invoice['number']}

Amount Due: {invoice['total_bth']:.8f} BTH
(Equivalent to ${invoice['total']:.2f} at rate {invoice['bth_rate']})

To pay, send exactly {invoice['total_bth']:.8f} BTH to:
Address: {invoice['payment_address']}
Memo: {invoice['payment_memo']}

Due Date: {invoice['due_date']}

Thank you for your business!
            """,
            attachments=[pdf]
        )
```

---

## Pricing and Exchange Rates

### Dynamic Pricing

```python
import requests
from functools import lru_cache
import time

class PricingEngine:
    def __init__(self, base_currency='USD'):
        self.base_currency = base_currency
        self.rate_cache = {}
        self.cache_ttl = 300  # 5 minutes

    def get_bth_rate(self):
        """Get current BTH/USD rate"""
        now = time.time()

        if 'bth' in self.rate_cache:
            rate, timestamp = self.rate_cache['bth']
            if now - timestamp < self.cache_ttl:
                return rate

        # Fetch from price API (when available)
        # For now, use placeholder
        rate = self.fetch_rate()
        self.rate_cache['bth'] = (rate, now)
        return rate

    def fetch_rate(self):
        """Fetch rate from price source"""
        # TODO: Implement when price feeds available
        # For testing, return fixed rate
        return 1.0  # 1 BTH = 1 USD

    def convert_to_bth(self, amount, currency='USD'):
        """Convert fiat amount to BTH"""
        rate = self.get_bth_rate()
        return amount / rate

    def convert_from_bth(self, bth_amount, currency='USD'):
        """Convert BTH to fiat"""
        rate = self.get_bth_rate()
        return bth_amount * rate
```

### Price Locking

For invoices, lock the exchange rate:

```python
def create_locked_invoice(self, amount_fiat, lock_duration=3600):
    """Create invoice with locked exchange rate"""
    rate = self.pricing.get_bth_rate()
    bth_amount = amount_fiat / rate

    invoice = {
        'amount_fiat': amount_fiat,
        'amount_bth': bth_amount,
        'locked_rate': rate,
        'rate_expires': time.time() + lock_duration,
        'status': 'pending'
    }

    return invoice

def is_rate_expired(self, invoice):
    """Check if locked rate has expired"""
    return time.time() > invoice['rate_expires']
```

---

## Refunds

Since BTH transactions are irreversible, handle refunds manually:

```python
class RefundManager:
    def __init__(self, rpc_url):
        self.rpc_url = rpc_url

    def process_refund(self, original_payment, refund_address, refund_amount):
        """Process a refund"""
        # Validate refund
        if refund_amount > original_payment['amount']:
            raise ValueError("Refund cannot exceed original payment")

        # Create refund transaction
        # This requires the wallet to have sufficient balance
        result = self.send_payment(refund_address, refund_amount)

        return {
            'refund_tx': result['tx_hash'],
            'amount': refund_amount,
            'original_payment': original_payment['tx_hash']
        }

    def send_payment(self, address, amount):
        """Send BTH payment"""
        # Use CLI or transaction builder
        import subprocess
        result = subprocess.run([
            'botho', 'send', address, str(amount)
        ], capture_output=True, text=True)

        if result.returncode != 0:
            raise Exception(f"Refund failed: {result.stderr}")

        return {'tx_hash': self.parse_tx_hash(result.stdout)}
```

---

## Security

### Payment Verification

Always verify payments server-side:

```python
def verify_payment(order_id, claimed_tx_hash):
    """Verify a payment is valid"""
    # 1. Check transaction exists
    # 2. Check amount matches invoice
    # 3. Check confirmations
    # 4. Check not already used

    payment = get_payment_by_order(order_id)

    if payment['tx_hash'] != claimed_tx_hash:
        return False

    if payment['confirmations'] < REQUIRED_CONFIRMATIONS:
        return False

    if payment['already_credited']:
        return False

    return True
```

### Rate Limiting

Protect against payment spam:

```python
from flask_limiter import Limiter

limiter = Limiter(app, key_func=get_remote_address)

@app.route('/payment/create')
@limiter.limit("10 per minute")
def create_payment():
    # Create payment request
    pass
```

### Webhook Security

If using webhooks for payment notifications:

```python
import hmac
import hashlib

def verify_webhook(payload, signature, secret):
    """Verify webhook signature"""
    expected = hmac.new(
        secret.encode(),
        payload.encode(),
        hashlib.sha256
    ).hexdigest()

    return hmac.compare_digest(expected, signature)

@app.route('/webhook/payment', methods=['POST'])
def payment_webhook():
    payload = request.get_data(as_text=True)
    signature = request.headers.get('X-Signature')

    if not verify_webhook(payload, signature, WEBHOOK_SECRET):
        return 'Invalid signature', 401

    # Process payment notification
    data = json.loads(payload)
    process_payment_notification(data)

    return 'OK', 200
```

---

## Testing

### Test Mode

Create a test mode for development:

```python
class BothoClient:
    def __init__(self, rpc_url, test_mode=False):
        self.rpc_url = rpc_url
        self.test_mode = test_mode

    def create_invoice(self, order_id, amount):
        if self.test_mode:
            return self.create_test_invoice(order_id, amount)
        return self.create_real_invoice(order_id, amount)

    def create_test_invoice(self, order_id, amount):
        """Create fake invoice for testing"""
        return {
            'order_id': order_id,
            'amount': amount,
            'address': 'TEST_ADDRESS',
            'memo': 'TEST_MEMO',
            'test_mode': True
        }

    def simulate_payment(self, order_id):
        """Simulate payment confirmation for testing"""
        if not self.test_mode:
            raise Exception("Can only simulate in test mode")

        payment = self.get_payment(order_id)
        payment['status'] = 'confirmed'
        payment['confirmations'] = 10
        payment['tx_hash'] = 'TEST_TX_HASH'

        return payment
```

---

## Checklist

### Before Launch

- [ ] Node running and synced
- [ ] Wallet backed up securely
- [ ] Payment detection working
- [ ] Confirmation counting accurate
- [ ] Exchange rate source configured
- [ ] Error handling tested
- [ ] Refund process documented

### Integration

- [ ] Payment page displays correctly
- [ ] QR codes scannable
- [ ] Status updates working
- [ ] Webhook notifications configured
- [ ] Database storing payment records
- [ ] Accounting integration ready

### Security

- [ ] HTTPS enabled
- [ ] Rate limiting configured
- [ ] Payment verification server-side
- [ ] Webhook signatures verified
- [ ] Sensitive data encrypted

---

## Support

### Resources

- [API Reference](api.md) — Complete RPC documentation
- [Developer Guide](developer-guide.md) — Integration examples
- [Troubleshooting](troubleshooting.md) — Common issues

### Getting Help

- GitHub Issues: [github.com/botho-project/botho/issues](https://github.com/botho-project/botho/issues)

---

## Related Documentation

- [Exchange Integration](exchange-integration.md) — For exchanges
- [Security Guide](security.md) — Security best practices
- [Deployment Guide](deployment.md) — Production deployment
