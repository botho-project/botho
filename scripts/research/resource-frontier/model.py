#!/usr/bin/env python3
"""Inactive illustrative resource accounting; not a protocol or price model."""
import csv
import hashlib
import io
import json
from decimal import Decimal, InvalidOperation, localcontext
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
PICO = 10**12
BLOCKS_YEAR = 6_307_200
SOURCES = ['botho/src/consensus/lottery.rs', 'botho/src/monetary.rs',
           'cluster-tax/src/monetary.rs', 'botho/src/block.rs']


def dec(value, *, positive=False):
    if isinstance(value, bool) or not isinstance(value, (str, int, Decimal)):
        raise ValueError('use exact decimal strings or integers, never floats')
    try:
        value = Decimal(value)
    except InvalidOperation as error:
        raise ValueError("invalid decimal encoding") from error
    if not value.is_finite() or value < 0 or (positive and value == 0):
        raise ValueError('invalid nonnegative finite decimal')
    return value


def uint(value, bits=64):
    if isinstance(value, bool) or not isinstance(value, int) or not 0 <= value < 1 << bits:
        raise ValueError("unsigned integer width exceeded")
    return value


def emission(height, supply_pico):
    """Source transcription, not a Rust kernel invocation or monetary forecast."""
    height, supply_pico = uint(height), uint(supply_pico, 128)
    # Restrict to a checked transcription domain: no Rust multiplication/cast overflow.
    if supply_pico * 200 >= 1 << 128:
        raise ValueError("supply outside checked transcription domain")
    epoch = height // BLOCKS_YEAR
    if epoch < 5:
        reward = 50 * PICO >> epoch
    else:
        reward = max(1, (supply_pico * 200 // 10000
                         + supply_pico * 50 // 10000) // BLOCKS_YEAR)
    if reward >= 1 << 64:
        raise ValueError("reward outside u64 transcription domain")
    lottery = reward * min(epoch * 1000, 5000) // 10000
    return dict(reward_pico=reward, miner_pico=reward-lottery, lottery_pico=lottery)


def fee_split(total_pico):
    total_pico = uint(total_pico)
    pool = total_pico * 800 // 1000
    return dict(gross_pico=total_pico, pool_pico=pool, burn_pico=total_pico-pool)


def funding(cost, transactions, price, capture, support):
    cost, transactions, price = dec(cost), dec(transactions), dec(price, positive=True)
    capture, support = dec(capture), dec(support)
    if capture > 1:
        raise ValueError('capture must be a fraction')
    deficit = max(Decimal(0), cost-support)
    if deficit == 0:
        return dict(status='covered_by_assigned_support', gap_currency=deficit, fee_bth=Decimal(0))
    if transactions == 0:
        return dict(status='no_accepted_transactions', gap_currency=deficit, fee_bth=None)
    if capture == 0:
        return dict(status='no_direct_fee_funding_under_no_indirect_revenue_assumption',
                    gap_currency=deficit, fee_bth=None)
    return dict(status='hypothetical_direct_capture', gap_currency=deficit,
                fee_bth=deficit/(transactions*price*capture))


def affordability(fee, payment, fraction):
    fee, payment, fraction = dec(fee), dec(payment, positive=True), dec(fraction, positive=True)
    if fraction > 1:
        raise ValueError('fraction greater than one')
    return dict(fee_payment_ratio=fee/payment, total_spend_bth=payment+fee,
                minimum_payment_at_supplied_fraction_bth=fee/fraction,
                within_supplied_fraction=fee <= payment*fraction)


def evaluate(preset, common, price, utilization, retention_hours):
    with localcontext() as context:
        context.prec = 50
        p = {k: dec(v) for k, v in preset.items() if k != 'name'}
        c = {k: dec(v) for k, v in common.items()}
        price, utilization = dec(price, positive=True), dec(utilization)
        if utilization > 1 or c['period_hours'] == 0 or c['capacity_tx_s'] == 0:
            raise ValueError('invalid utilization/period/capacity')
        if p['replicas'] < 1 or p['replicas'] != p['replicas'].to_integral_value():
            raise ValueError('replicas must be positive integers')
        if p['cores_per_replica'] == 0:
            raise ValueError('positive replica compute capacity required')
        if c['work_multiplier'] < 1:
            raise ValueError('work multiplier includes accepted verification')
        h = c['period_hours']
        n = h*3600*c['capacity_tx_s']*utilization
        work = n*c['work_multiplier']
        cpu_seconds = work*c['verify_seconds']
        compute_load = cpu_seconds/(h*3600*p['cores_per_replica']) if p['cores_per_replica'] else Decimal('Infinity')
        status = 'overloaded' if compute_load > 1 else 'evaluated'
        if retention_hours is None:
            return dict(status='nonstationary_append_only_storage', accepted_transactions=n,
                        note='Finite-horizon append-only cost is possible; no finite stationary stock is modeled.')
        retention = dec(retention_hours)
        # One GB is exactly 10^9 bytes. Little's law stock: arrival/hour * retention.
        storage_gb = n/h*retention*c['transaction_bytes']*p['replicas']/Decimal(10**9)
        components = dict(
            consensus_electricity=p['consensus_network_kw']*h*c['electricity_currency_kwh'],
            consensus_fixed=p['consensus_fixed_currency_hour']*h,
            verifier_base_electricity=p['replicas']*p['verifier_base_kw']*h*c['electricity_currency_kwh'],
            verifier_incremental_electricity=p['replicas']*cpu_seconds*c['verify_incremental_kw']/3600*c['electricity_currency_kwh'],
            verifier_cpu_amortization=p['replicas']*cpu_seconds*c['cpu_currency_second'],
            verifier_fixed=p['replicas']*p['verifier_fixed_currency_hour']*h,
            bandwidth=n*c['transaction_bytes']*p['delivery_copies']/Decimal(10**9)*c['bandwidth_currency_gb'],
            storage=storage_gb*h*c['storage_currency_gb_hour'],
            fixed_stock_storage=p['fixed_stock_gb']*h*c['storage_currency_gb_hour'],
        )
        consensus = components['consensus_electricity']+components['consensus_fixed']
        verifier = sum(components.values())-consensus
        total = consensus+verifier
        security_extra = p['additional_security_currency_hour']*h
        support_bth = p['assigned_consensus_issuance_bth_hour']*h
        # Available miner issuance from explicit policy state, not an unconstrained subsidy.
        issuance = emission(common['issuance_height'], common['issuance_supply_pico'])
        interval = dec(common['actual_block_seconds'], positive=True)
        available_bth = Decimal(issuance['miner_pico'])/PICO * (h*3600/interval)
        if support_bth > available_bth:
            raise ValueError('assigned issuance exceeds available miner issuance')
        # Only consensus role receives this explicitly assigned issuance support.
        supported_gap = max(Decimal(0),consensus-support_bth*price)+verifier
        frontier = total/(n*price) if n and status == 'evaluated' else None
        scenarios = {}
        for capture in [Decimal(0), Decimal('0.5'), Decimal(1)]:
            scenarios[str(capture)] = dict(
                fee_only=funding(total,n,price,capture,0),
                issuance_supported=funding(supported_gap,n,price,capture,0),
                incremental_security=funding(security_extra,n,price,capture,0),
                hypothetical_routing=dict(operator=capture, lottery=(1-capture)*Decimal('0.8'),
                                          burn=(1-capture)*Decimal('0.2')),
            )
        if status == 'overloaded':
            for row in scenarios.values():
                for key in ['fee_only','issuance_supported','incremental_security']:
                    row[key]['fee_bth']=None
                    row[key]['status']='overloaded_no_feasible_throughput_claim'
        return dict(status=status, accepted_transactions=n, verification_work_units=work,
                    replica_compute_utilization=compute_load, stationary_storage_gb=storage_gb,
                    components_currency=components, total_resource_currency=total,
                    consensus_resource_currency=consensus, verifier_resource_currency=verifier,
                    available_miner_issuance_bth=available_bth,
                    assigned_consensus_issuance_bth=support_bth,
                    unassigned_miner_issuance_bth=available_bth-support_bth,
                    unused_consensus_support_currency=max(Decimal(0),support_bth*price-consensus),
                    remaining_role_separated_gap_currency=supported_gap,
                    additional_security_currency=security_extra,
                    resource_equivalent_fee_bth=frontier,
                    resource_plus_additional_security_fee_bth=(total+security_extra)/(n*price) if frontier is not None else None,
                    funding=scenarios)


def serial(value):
    if isinstance(value, Decimal):
        return format(value, 'f')
    if isinstance(value, dict):
        return {k:serial(v) for k,v in value.items()}
    if isinstance(value, list):
        return [serial(v) for v in value]
    return value


def csv_text(rows):
    stream=io.StringIO(newline='')
    writer=csv.DictWriter(stream, fieldnames=list(rows[0]), lineterminator='\n')
    writer.writeheader()
    writer.writerows(serial(rows))
    return stream.getvalue()


def reports(config):
    rows=[]; flat=[]; affordability_rows=[]
    for preset in config['presets']:
        for price in config['prices_currency_per_bth']:
            for utilization in config['utilizations']:
                for days in config['retention_days']:
                    ident=f"{preset['name']}-p{price}-u{utilization}-d{days}"
                    result=evaluate(preset,config['common'],price,utilization,dec(days)*24)
                    rows.append(dict(id=ident,preset=preset['name'],price_currency_per_bth=price,
                                     utilization=utilization,retention_days=days,**result))
                    fee=result['resource_equivalent_fee_bth']
                    flat.append(dict(id=ident,status=result['status'],transactions=result['accepted_transactions'],
                                     cost_currency=result['total_resource_currency'],fee_bth=fee,
                                     security_extra_currency=result['additional_security_currency']))
                    if fee is not None:
                        for payment in config['payment_sizes_bth']:
                            for fraction in config['payment_fractions']:
                                affordability_rows.append(dict(id=ident,payment_bth=payment,supplied_fraction=fraction,
                                                               fee_bth=fee,**affordability(fee,payment,fraction)))
    sentinel=evaluate(config['presets'][0],config['common'],'1','0.1',None)
    sources={p:hashlib.sha256((ROOT/p).read_bytes()).hexdigest() for p in SOURCES}
    examples=[]
    for height in [0,BLOCKS_YEAR,5*BLOCKS_YEAR]:
        e=emission(height,611_010_000*PICO)
        for seconds in [5,40]:
            examples.append(dict(height=height,supply_pico=611_010_000*PICO,actual_block_seconds=seconds,
                                 **e,miner_bth_hour=Decimal(e['miner_pico'])/PICO*3600/seconds))
    doc=dict(base_commit=config['base_commit'],input_status='illustrative, unmeasured; no price forecasts',
             source_sha256=sources,model_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
             canonical_config_sha256=hashlib.sha256(json.dumps(config,sort_keys=True,separators=(',',':')).encode()).hexdigest(),
             actual_config=config,
             emission_transcription_examples=examples,current_fee_split_examples=[fee_split(v) for v in [0,1,999,10**12]],
             rows=rows,unbounded_retention_sentinel=sentinel)
    return {'report.json':json.dumps(serial(doc),indent=2)+'\n','frontier.csv':csv_text(flat),
            'affordability.csv':csv_text(affordability_rows)}


def main():
    config=json.loads((HERE/'scenarios.json').read_text())
    for name,content in reports(config).items(): (HERE/name).write_text(content)


if __name__=='__main__':
    main()
