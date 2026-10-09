# Inventory

The parts you have in stock, at your own sites and at partners. Inventory tracks who owns stock, who holds it, where it is, and what it can be used for. It never claims more precision than the counts support.

## Language

### Stock

**Stock pool**:
All of one part's stock that shares the same owner, holder, site, status and restrictions. It is the smallest amount you can reserve on its own.
_Avoid_: Stock Holding, lot, bin

**Container**:
A physical reel, tray or bag of stock, with its lot and date code. Recording containers is optional.
_Avoid_: Stock Container, package

**Partner count**:
A quantity a partner reports, with when and how they reported it and how sure it is. It never invents reels or lots the partner did not report.
_Avoid_: Partner Stock Balance, partner reel

**Confidence**:
How sure a quantity is, from an exact count to an estimate such as a part-used reel.
_Avoid_: Quantity Confidence, accuracy

**In transit**:
Stock that has left one site and not yet arrived at another. It is counted once, in one place.
_Avoid_: In-Transit Stock, shipped stock

### Who and where

**Owner**:
Who owns the stock.
_Avoid_: Recorded Owner, vendor

**Holder**:
Who physically has the stock, which may not be the owner.
_Avoid_: Custodian, location

**Site**:
A place where stock is kept, such as your lab or an assembly house.
_Avoid_: Inventory Site, warehouse

**Location**:
A spot within a site, such as a shelf or bin.
_Avoid_: Storage Location

**Supplier**:
Where the stock was bought or came from.
_Avoid_: Stock Source, vendor, owner

### Using stock

**Status**:
Whether stock can be used right now: available, quarantined or in transit.
_Avoid_: Stock State

**Restriction**:
A limit on what stock may be used for, such as stock kept for one customer's builds.
_Avoid_: Use Constraint, flag, tag

**Stock move**:
A recorded change to the quantity, owner, holder, site or location of stock. Reservations and minimum stock are never stock moves.
_Avoid_: Stock Movement, transaction, adjustment

**Demand**:
A specific need for parts, such as a number of builds of one KiCad project.
_Avoid_: Named Demand, requirement, order

**Reservation**:
A time-limited claim on stock for one demand. It does not change the quantity in stock.
_Avoid_: Material Reservation, allocation, hold

**Minimum stock**:
A standing amount to keep at a site, such as "Keep local". It does not change the quantity in stock.
_Avoid_: Stock Floor, safety stock, buffer

**Available for a build**:
How much stock one demand can use after statuses, restrictions, other reservations and minimum stock. There is no single "available" number for every build.
_Avoid_: Allocatable Quantity, free stock
