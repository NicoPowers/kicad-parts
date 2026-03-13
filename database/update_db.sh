#!/usr/bin/env bash

# Script to create/update the parts database from CSV files


# define all of your libs here -- should be a CSV file for each lib
GPLMLIBS="ana art cap con cpd dio ics ind mcu mec mpu opt osc pwr reg res rfm rvr swi trs"

DBFILE=./parts.sqlite

for lib in ${GPLMLIBS}; do
	sqlite3 ${DBFILE} "DROP TABLE IF EXISTS ${lib}" || return 1
	sqlite3 --csv ${DBFILE} ".import ./g-${lib}.csv ${lib}" || return 1
done

